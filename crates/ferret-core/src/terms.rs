//! Query parsing: several words, and wildcards.
//!
//! `rapor pdf` should find a file whose name contains both, in either order,
//! and `*.pdf` should mean what everyone expects it to mean. Both are what
//! people already type, so both are what the query understands.
//!
//! Splitting on spaces has one cost: a file with a space in its name can no
//! longer be found by typing that space. Quoting it — `"annual report"` — asks
//! for the phrase back.

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

/// A `*`/`?` pattern, pre-split into the literal runs between its wildcards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wildcard {
    /// Literal runs, in order. Empty strings mean adjacent wildcards.
    parts: Vec<String>,
    /// Must the first part sit at the very start of the name?
    anchored_start: bool,
    /// Must the last part sit at the very end?
    anchored_end: bool,
    /// Positions of `?`, which consume exactly one character each.
    single: Vec<usize>,
}

impl Wildcard {
    /// `?` is handled by matching lengths rather than by building a state
    /// machine: it is rare in practice, and a character-by-character walk is
    /// easy to be sure of.
    pub fn matches(&self, haystack: &str) -> bool {
        matches_pattern(&self.pattern_chars(), haystack)
    }

    fn pattern_chars(&self) -> Vec<char> {
        // Reassemble the original pattern; cheap, and keeps one matcher.
        let mut out = Vec::new();
        for (i, part) in self.parts.iter().enumerate() {
            if i > 0 {
                out.push('*');
            }
            out.extend(part.chars());
        }
        if !self.anchored_start {
            out.insert(0, '*');
        }
        if !self.anchored_end {
            out.push('*');
        }
        for _ in &self.single {}
        out
    }

    pub fn longest_literal(&self) -> Option<&str> {
        self.parts
            .iter()
            .filter(|part| !part.is_empty() && !part.contains('?'))
            .max_by_key(|part| part.len())
            .map(String::as_str)
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
    let anchored_start = !token.starts_with('*');
    let anchored_end = !token.ends_with('*');
    let parts: Vec<String> = token.split('*').map(str::to_string).collect();
    let single = token
        .char_indices()
        .filter(|(_, c)| *c == '?')
        .map(|(i, _)| i)
        .collect();

    Wildcard {
        parts,
        anchored_start,
        anchored_end,
        single,
    }
}

/// Classic wildcard matching with backtracking.
///
/// Iterative rather than recursive: a pathological pattern like `*a*a*a*a*` on
/// a long name should get slow, not blow the stack.
fn matches_pattern(pattern: &[char], text: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut s) = (0usize, 0usize);
    let (mut star, mut resume) = (None, 0usize);

    while s < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[s]) {
            p += 1;
            s += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
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

    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
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
}
