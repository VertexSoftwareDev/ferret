//! Query parsing: several words, and wildcards.
//!
//! `rapor pdf` should find a file whose name contains both, in either order,
//! and `*.pdf` should mean what everyone expects it to mean. Both are what
//! people already type, so both are what the query understands.
//!
//! Splitting on spaces has one cost: a file with a space in its name can no
//! longer be found by typing that space. Quoting it — `"annual report"` — asks
//! for the phrase back.
//!
//! Everything a match needs is worked out once, at parse time. A wildcard query
//! can be tested against a hundred thousand candidate names, so a matcher that
//! allocated per call would cost more than the arena scan that found them.

/// One condition a name has to satisfy. All of them must, in any order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// Plain substring.
    Contains(String),
    /// `*` matches any run of characters, `?` exactly one.
    Wildcard(Wildcard),
}

impl Term {
    /// Whether `haystack` — which must already be lowercase — satisfies this.
    pub fn matches(&self, haystack: &str) -> bool {
        match self {
            Term::Contains(needle) => haystack.contains(needle.as_str()),
            Term::Wildcard(pattern) => pattern.matches(haystack),
        }
    }

    /// The longest literal run inside the term, for use as the scan needle.
    ///
    /// Scanning 50 MB for the rarest piece of the query and checking the rest
    /// against the survivors is far cheaper than testing every name.
    pub fn anchor(&self) -> Option<&str> {
        match self {
            Term::Contains(needle) => Some(needle.as_str()),
            Term::Wildcard(pattern) => pattern.longest_literal(),
        }
    }
}

/// The shapes worth recognising, because almost every real wildcard query is
/// one of them and none of them need a backtracking matcher.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shape {
    /// `*text`
    EndsWith(String),
    /// `text*`
    StartsWith(String),
    /// `*text*`
    Contains(String),
    /// Anything else: run the general matcher.
    General,
}

/// A compiled `*`/`?` pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wildcard {
    /// The pattern as characters, ready to match without re-parsing.
    pattern: Vec<char>,
    /// The same as bytes, when the pattern is pure ASCII — which lets an ASCII
    /// name be matched without turning it into a `Vec<char>` first.
    ascii: Option<Vec<u8>>,
    /// Literal runs between the wildcards, longest first, for [`Term::anchor`].
    literals: Vec<String>,
    shape: Shape,
}

impl Wildcard {
    pub fn matches(&self, haystack: &str) -> bool {
        match &self.shape {
            Shape::EndsWith(text) => haystack.ends_with(text.as_str()),
            Shape::StartsWith(text) => haystack.starts_with(text.as_str()),
            Shape::Contains(text) => haystack.contains(text.as_str()),
            Shape::General => match &self.ascii {
                // The common case: neither side needs a character vector.
                Some(pattern) if haystack.is_ascii() => matches_bytes(pattern, haystack.as_bytes()),
                _ => {
                    let text: Vec<char> = haystack.chars().collect();
                    matches_chars(&self.pattern, &text)
                }
            },
        }
    }

    pub fn longest_literal(&self) -> Option<&str> {
        self.literals.first().map(String::as_str)
    }
}

/// Split a query into terms.
///
/// Whitespace separates them, double quotes group one back together, and a term
/// containing `*` or `?` becomes a wildcard. The text is lowercased here so the
/// matcher never has to.
pub fn parse(text: &str) -> Vec<Term> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut quoted = false;

    let flush = |current: &mut String, terms: &mut Vec<Term>| {
        if current.is_empty() {
            return;
        }
        let token = std::mem::take(current);
        terms.push(if token.contains('*') || token.contains('?') {
            Term::Wildcard(compile(&token))
        } else {
            Term::Contains(token)
        });
    };

    for ch in text.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                // Closing a quote ends the term even if the next char is not a
                // space, so `"a b"c` is two terms rather than one odd one.
                if !quoted {
                    flush(&mut current, &mut terms);
                }
            }
            c if c.is_whitespace() && !quoted => flush(&mut current, &mut terms),
            c => current.extend(c.to_lowercase()),
        }
    }
    flush(&mut current, &mut terms);

    terms
}

fn compile(token: &str) -> Wildcard {
    let pattern: Vec<char> = token.chars().collect();

    let mut literals: Vec<String> = token
        .split(['*', '?'])
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    literals.sort_by_key(|part| std::cmp::Reverse(part.len()));

    Wildcard {
        shape: shape_of(token),
        ascii: token.is_ascii().then(|| token.bytes().collect()),
        pattern,
        literals,
    }
}

/// Recognise the three shapes that need no backtracking at all.
fn shape_of(token: &str) -> Shape {
    // A `?` anywhere, or a `*` in the middle, means the general matcher.
    if token.contains('?') {
        return Shape::General;
    }
    let inner = token.trim_matches('*');
    if inner.is_empty() || inner.contains('*') {
        return Shape::General;
    }

    match (token.starts_with('*'), token.ends_with('*')) {
        (true, true) => Shape::Contains(inner.to_string()),
        (true, false) => Shape::EndsWith(inner.to_string()),
        (false, true) => Shape::StartsWith(inner.to_string()),
        // No star at all cannot happen here, but an exact match is still right.
        (false, false) => Shape::General,
    }
}

/// Classic wildcard matching with backtracking, over bytes.
///
/// Iterative rather than recursive: a pathological pattern like `*a*a*a*a*` on
/// a long name should get slow, not blow the stack.
fn matches_bytes(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut s) = (0usize, 0usize);
    let (mut star, mut resume) = (None, 0usize);

    while s < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[s]) {
            p += 1;
            s += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            // Remember where to come back to if the rest fails to line up.
            star = Some(p);
            resume = s;
            p += 1;
        } else if let Some(at) = star {
            // Let the last star swallow one more character and try again.
            p = at + 1;
            resume += 1;
            s = resume;
        } else {
            return false;
        }
    }

    pattern[p..].iter().all(|c| *c == b'*')
}

/// The same algorithm for names that are not ASCII.
fn matches_chars(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut s) = (0usize, 0usize);
    let (mut star, mut resume) = (None, 0usize);

    while s < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[s]) {
            p += 1;
            s += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            resume = s;
            p += 1;
        } else if let Some(at) = star {
            p = at + 1;
            resume += 1;
            s = resume;
        } else {
            return false;
        }
    }

    pattern[p..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains(text: &str) -> Term {
        Term::Contains(text.to_string())
    }

    #[test]
    fn splits_on_whitespace_and_lowercases() {
        assert_eq!(parse("Rapor PDF"), vec![contains("rapor"), contains("pdf")]);
        assert_eq!(
            parse("   spaced   out  "),
            vec![contains("spaced"), contains("out")]
        );
        assert_eq!(parse(""), vec![]);
    }

    #[test]
    fn quotes_hold_a_phrase_together() {
        assert_eq!(parse("\"yıllık rapor\""), vec![contains("yıllık rapor")]);
        assert_eq!(
            parse("\"yıllık rapor\" pdf"),
            vec![contains("yıllık rapor"), contains("pdf")]
        );
    }

    #[test]
    fn every_term_has_to_match() {
        let terms = parse("rapor pdf");
        assert!(terms.iter().all(|term| term.matches("2026 rapor son.pdf")));
        // Order does not matter.
        assert!(terms.iter().all(|term| term.matches("pdf-rapor")));
        // One missing is enough to reject.
        assert!(!terms.iter().all(|term| term.matches("rapor.docx")));
    }

    #[test]
    fn star_matches_any_run() {
        let terms = parse("*.pdf");
        assert!(terms[0].matches("rapor.pdf"));
        assert!(terms[0].matches(".pdf"));
        assert!(!terms[0].matches("rapor.pdf.bak"));
        assert!(!terms[0].matches("rapor.docx"));
    }

    #[test]
    fn question_mark_matches_exactly_one() {
        let terms = parse("rapor?.txt");
        assert!(terms[0].matches("rapor1.txt"));
        assert!(!terms[0].matches("rapor.txt"));
        assert!(!terms[0].matches("rapor12.txt"));
    }

    #[test]
    fn a_pattern_without_edge_stars_is_anchored() {
        // "a*b" means the whole name, not a fragment of it.
        let terms = parse("a*b");
        assert!(terms[0].matches("ab"));
        assert!(terms[0].matches("axxb"));
        assert!(!terms[0].matches("xaby"));
    }

    #[test]
    fn stars_can_be_repeated_and_adjacent() {
        let terms = parse("**rapor**");
        assert!(terms[0].matches("bir rapor var"));
        assert!(!terms[0].matches("bir belge var"));
    }

    #[test]
    fn a_backtracking_pattern_terminates() {
        // The shape that makes naive matchers hang.
        let terms = parse("*a*a*a*a*a*b");
        assert!(!terms[0].matches(&"a".repeat(60)));
        assert!(terms[0].matches(&format!("{}b", "a".repeat(30))));
    }

    #[test]
    fn the_anchor_is_the_longest_literal_run() {
        assert_eq!(parse("rapor")[0].anchor(), Some("rapor"));
        assert_eq!(parse("*yillik*rapor*")[0].anchor(), Some("yillik"));
        assert_eq!(parse("*.pdf")[0].anchor(), Some(".pdf"));
        // Nothing literal to scan for.
        assert_eq!(parse("*")[0].anchor(), None);
        assert_eq!(parse("?")[0].anchor(), None);
    }

    #[test]
    fn wildcards_are_case_insensitive_through_parsing() {
        let terms = parse("*.PDF");
        assert!(terms[0].matches("rapor.pdf"));
    }

    #[test]
    fn non_ascii_names_take_the_character_path() {
        let terms = parse("*rapor*");
        assert!(terms[0].matches("çalışma raporu.docx"));
        assert!(!terms[0].matches("çalışma özeti.docx"));

        // A non-ASCII pattern too.
        let turkish = parse("*özet*");
        assert!(turkish[0].matches("yıllık özet.pdf"));
        assert!(!turkish[0].matches("yıllık rapor.pdf"));
    }

    /// The shortcuts must agree with the general matcher, always.
    #[test]
    fn the_fast_shapes_agree_with_the_general_matcher() {
        let names = [
            "rapor.pdf",
            ".pdf",
            "rapor.pdf.bak",
            "pdf",
            "",
            "a",
            "rapor",
            "çalışma raporu.pdf",
        ];
        for token in ["*.pdf", "rapor*", "*rapor*", "*", "**"] {
            let compiled = compile(token);
            let general = Wildcard {
                shape: Shape::General,
                ..compiled.clone()
            };
            for name in names {
                assert_eq!(
                    compiled.matches(name),
                    general.matches(name),
                    "'{token}' vs '{name}'"
                );
            }
        }
    }

    #[test]
    fn the_byte_and_character_matchers_agree() {
        for token in ["*.pdf", "a*b", "rapor?.txt", "*a*a*b", "?", "*"] {
            let compiled = compile(token);
            let pattern: Vec<char> = token.chars().collect();
            for name in ["rapor.pdf", "ab", "axxb", "rapor1.txt", "aaab", "x", ""] {
                let chars: Vec<char> = name.chars().collect();
                assert_eq!(
                    matches_bytes(token.as_bytes(), name.as_bytes()),
                    matches_chars(&pattern, &chars),
                    "'{token}' vs '{name}'"
                );
                // And the compiled form agrees with both.
                assert_eq!(
                    compiled.matches(name),
                    matches_chars(&pattern, &chars),
                    "compiled '{token}' vs '{name}'"
                );
            }
        }
    }
}
