//! Instant substring search over an [`Index`].
//!
//! The naive version — lowercase every name on every keystroke — costs about
//! 100 ms on a 1.8 M file volume, because it allocates a fresh string per file
//! per query. That is far too slow to feel live while typing.
//!
//! So the lowercase form is computed **once**, at build time, into a single
//! contiguous arena with `\n` between names. A query then becomes one linear
//! scan of ~50 MB of bytes, which the two-way matcher behind `str::find` chews
//! through at gigabytes per second, split across all cores. NTFS forbids
//! control characters in filenames, so `\n` is a separator that can never occur
//! inside a name.
//!
//! Path queries (`users\pc\rap`) are handled without ever materialising a
//! million paths: only the last segment is matched against the name arena, and
//! the few entries that survive have their path built and checked.

use std::thread;

use crate::mft::{filetime_to_unix, Entry, Index};

/// Byte that separates names in the arena; illegal inside an NTFS filename.
const SEPARATOR: u8 = b'\n';

/// Below this many entries the thread hand-off costs more than it saves.
const PARALLEL_THRESHOLD: usize = 50_000;

/// One hit: the position of the entry inside [`Index::entries`].
pub type Hit = u32;

/// A prepared, lowercase copy of every name in an index.
///
/// Built once per scan; queried as often as the user types.
pub struct SearchIndex {
    /// All names, lowercased, joined by [`SEPARATOR`].
    haystack: String,
    /// `starts[i]` is where entry `i`'s name begins in `haystack`.
    starts: Vec<u32>,
}

/// How results should be ordered.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    /// Order the records appear in the MFT — free, and roughly by age.
    #[default]
    None,
    Name,
    Size,
    Modified,
    Path,
}

/// Which way a sort runs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

/// Filters applied on top of a text match.
#[derive(Debug, Default, Clone, Copy)]
pub struct Filter {
    pub files_only: bool,
    pub dirs_only: bool,
    /// Keep entries at least this large, in bytes.
    pub min_size: Option<u64>,
    /// Keep entries no larger than this, in bytes.
    pub max_size: Option<u64>,
    /// Keep entries modified at or after this Unix timestamp.
    pub modified_after: Option<i64>,
    /// Drop entries carrying the hidden or system attribute.
    pub skip_hidden: bool,
}

impl Filter {
    pub fn keeps(&self, entry: &Entry) -> bool {
        if self.files_only && entry.is_dir() {
            return false;
        }
        if self.dirs_only && !entry.is_dir() {
            return false;
        }
        if self.skip_hidden && (entry.is_hidden() || entry.is_system()) {
            return false;
        }
        if let Some(min) = self.min_size {
            if entry.size < min {
                return false;
            }
        }
        if let Some(max) = self.max_size {
            if entry.size > max {
                return false;
            }
        }
        if let Some(after) = self.modified_after {
            match filetime_to_unix(entry.modified) {
                Some(seconds) if seconds >= after => {}
                _ => return false,
            }
        }
        true
    }

    fn is_noop(&self) -> bool {
        !self.files_only
            && !self.dirs_only
            && !self.skip_hidden
            && self.min_size.is_none()
            && self.max_size.is_none()
            && self.modified_after.is_none()
    }
}

/// Everything a caller wants to ask of the index in one go.
#[derive(Debug, Default, Clone)]
pub struct Query {
    pub text: String,
    pub filter: Filter,
    pub sort_by: SortBy,
    pub order: SortOrder,
}

impl SearchIndex {
    /// Prepare the lowercase arena for an index.
    pub fn build(index: &Index) -> SearchIndex {
        let entries = index.entries();
        // Names average out around 20 bytes; over-reserving beats regrowing a
        // 50 MB string a dozen times.
        let mut haystack = String::with_capacity(entries.len() * 24);
        let mut starts = Vec::with_capacity(entries.len());

        for position in 0..entries.len() {
            starts.push(haystack.len() as u32);
            push_lowercase(&mut haystack, index.name(position));
            haystack.push(SEPARATOR as char);
        }

        haystack.shrink_to_fit();
        SearchIndex { haystack, starts }
    }

    /// Bytes held by the arena, for reporting memory use.
    pub fn memory_bytes(&self) -> usize {
        self.haystack.len() + self.starts.len() * std::mem::size_of::<u32>()
    }

    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    /// Find every entry whose name contains `needle`, case-insensitively.
    ///
    /// Results come back in index order. An empty needle matches everything.
    pub fn search(&self, needle: &str) -> Vec<Hit> {
        if needle.is_empty() {
            return (0..self.starts.len() as u32).collect();
        }
        let needle = needle.to_lowercase();

        // A needle containing the separator can never match a single name.
        if needle.as_bytes().contains(&SEPARATOR) {
            return Vec::new();
        }

        let threads = if self.starts.len() < PARALLEL_THRESHOLD {
            1
        } else {
            thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        };

        if threads <= 1 {
            return self.search_range(&needle, 0, self.starts.len());
        }

        // Split by entry so no name is cut in half across two workers.
        let per_thread = self.starts.len().div_ceil(threads);
        let mut parts: Vec<Vec<Hit>> = thread::scope(|scope| {
            let mut handles = Vec::with_capacity(threads);
            for t in 0..threads {
                let begin = t * per_thread;
                if begin >= self.starts.len() {
                    break;
                }
                let end = (begin + per_thread).min(self.starts.len());
                let needle = needle.as_str();
                handles.push(scope.spawn(move || self.search_range(needle, begin, end)));
            }
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });

        // Workers own disjoint, ascending ranges, so concatenating keeps order.
        let total: usize = parts.iter().map(Vec::len).sum();
        let mut hits = Vec::with_capacity(total);
        for part in parts.iter_mut() {
            hits.append(part);
        }
        hits
    }

    /// Run a full query: text match, path match, filters and sorting.
    pub fn run(&self, index: &Index, query: &Query) -> Vec<Hit> {
        let text = query.text.trim();

        // A query with a separator in it means "match the path", so the name
        // arena is searched for the final segment only and the survivors —
        // usually a handful — get their full path built and checked.
        let (needle, path_needle) = split_path_query(text);

        let mut hits = self.search(needle);

        if let Some(path_needle) = path_needle {
            // Paths are built with backslashes, but people type either slash,
            // so the needle is normalised rather than the million paths.
            let lowered = path_needle.to_lowercase().replace('/', "\\");
            let mut buffer = String::with_capacity(96);
            hits.retain(|hit| {
                if !index.write_full_path(*hit as usize, &mut buffer) {
                    return false;
                }
                buffer.to_lowercase().contains(&lowered)
            });
        }

        if !query.filter.is_noop() {
            let entries = index.entries();
            hits.retain(|hit| match entries.get(*hit as usize) {
                Some(entry) => query.filter.keeps(entry),
                None => false,
            });
        }

        sort_hits(index, &mut hits, query.sort_by, query.order);
        hits
    }

    /// Scan the slice of the arena covering entries `begin..end`.
    fn search_range(&self, needle: &str, begin: usize, end: usize) -> Vec<Hit> {
        let from = self.starts[begin] as usize;
        let to = if end < self.starts.len() {
            self.starts[end] as usize
        } else {
            self.haystack.len()
        };

        let slab = &self.haystack[from..to];
        let mut hits = Vec::new();
        let mut cursor = 0usize;

        while cursor < slab.len() {
            let Some(found) = slab[cursor..].find(needle) else {
                break;
            };
            let absolute = from + cursor + found;
            let entry = self.entry_at(absolute, begin, end);
            hits.push(entry as Hit);

            // One hit per name is enough: jump past this name's separator so a
            // second occurrence inside the same filename is not reported twice.
            let name_end = self.name_end(entry);
            cursor = (name_end + 1).saturating_sub(from).max(cursor + 1);
        }

        hits
    }

    /// Which entry owns the byte at `offset`.
    fn entry_at(&self, offset: usize, begin: usize, end: usize) -> usize {
        // starts is sorted, so the owner is the last start at or before offset.
        let window = &self.starts[begin..end];
        let position = window.partition_point(|s| (*s as usize) <= offset);
        begin + position.saturating_sub(1)
    }

    /// Offset of the separator that closes entry `i`.
    fn name_end(&self, i: usize) -> usize {
        if i + 1 < self.starts.len() {
            self.starts[i + 1] as usize - 1
        } else {
            self.haystack.len().saturating_sub(1)
        }
    }
}

/// Split `users\pc\rep` into the name needle (`rep`) and the whole string,
/// which the path check then has to contain.
fn split_path_query(text: &str) -> (&str, Option<&str>) {
    match text.rfind(['\\', '/']) {
        Some(cut) => (&text[cut + 1..], Some(text)),
        None => (text, None),
    }
}

/// Order a hit list in place. Exposed so a caller can match first and decide
/// about sorting afterwards, which is what keeps a half-million-hit query fast.
pub fn sort_hits(index: &Index, hits: &mut [Hit], sort_by: SortBy, order: SortOrder) {
    if sort_by == SortBy::None || hits.len() < 2 {
        return;
    }
    let entries = index.entries();

    match sort_by {
        SortBy::None => {}
        SortBy::Name => hits.sort_unstable_by(|a, b| {
            let left = index.name(*a as usize);
            let right = index.name(*b as usize);
            compare_case_insensitive(left, right).then(a.cmp(b))
        }),
        SortBy::Size => hits.sort_unstable_by(|a, b| {
            let left = entries.get(*a as usize).map(|e| e.size).unwrap_or(0);
            let right = entries.get(*b as usize).map(|e| e.size).unwrap_or(0);
            left.cmp(&right).then(a.cmp(b))
        }),
        SortBy::Modified => hits.sort_unstable_by(|a, b| {
            let left = entries.get(*a as usize).map(|e| e.modified).unwrap_or(0);
            let right = entries.get(*b as usize).map(|e| e.modified).unwrap_or(0);
            left.cmp(&right).then(a.cmp(b))
        }),
        SortBy::Path => {
            // Building a path per comparison would be O(n log n) path walks, so
            // each path is built once and sorted alongside its hit.
            let mut keyed: Vec<(String, Hit)> = hits
                .iter()
                .map(|h| (index.full_path(*h as usize).unwrap_or_default(), *h))
                .collect();
            keyed.sort_unstable_by(|a, b| compare_case_insensitive(&a.0, &b.0).then(a.1.cmp(&b.1)));
            for (slot, (_, hit)) in hits.iter_mut().zip(keyed) {
                *slot = hit;
            }
        }
    }

    if order == SortOrder::Descending {
        hits.reverse();
    }
}

/// Compare two names the way a file manager does: case-insensitively, but
/// still deterministically for names that differ only in case.
fn compare_case_insensitive(left: &str, right: &str) -> std::cmp::Ordering {
    let mut a = left.chars().flat_map(char::to_lowercase);
    let mut b = right.chars().flat_map(char::to_lowercase);
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) => match x.cmp(&y) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            },
            (None, None) => return left.cmp(right),
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
        }
    }
}

/// Append `text` lowercased.
///
/// ASCII is the overwhelmingly common case and is folded without touching the
/// Unicode tables; anything else falls back to full Unicode lowercasing.
fn push_lowercase(out: &mut String, text: &str) {
    if text.is_ascii() {
        out.extend(text.chars().map(|c| c.to_ascii_lowercase()));
    } else {
        out.extend(text.chars().flat_map(char::to_lowercase));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mft::test_support::{dir, file, index_from_names, index_from_specs};
    use crate::mft::ROOT_RECORD;

    fn names_of<'a>(index: &'a Index, hits: &[Hit]) -> Vec<&'a str> {
        hits.iter().map(|h| index.name(*h as usize)).collect()
    }

    #[test]
    fn finds_substrings_case_insensitively() {
        let index = index_from_names(&["Rapor.pdf", "notlar.txt", "RAPORLAR", "foto.jpg"]);
        let search = SearchIndex::build(&index);

        assert_eq!(
            names_of(&index, &search.search("rapor")),
            vec!["Rapor.pdf", "RAPORLAR"]
        );
        assert_eq!(
            names_of(&index, &search.search("RAPOR")),
            vec!["Rapor.pdf", "RAPORLAR"]
        );
        assert_eq!(names_of(&index, &search.search(".txt")), vec!["notlar.txt"]);
    }

    #[test]
    fn reports_each_name_once_even_with_repeats() {
        let index = index_from_names(&["aaa.txt", "b.txt"]);
        let search = SearchIndex::build(&index);
        assert_eq!(search.search("a").len(), 1);
    }

    #[test]
    fn a_match_never_spans_two_names() {
        // "xy" only exists if the end of one name runs into the next.
        let index = index_from_names(&["file_x", "y_file"]);
        let search = SearchIndex::build(&index);
        assert!(search.search("xy").is_empty());
    }

    #[test]
    fn empty_needle_matches_everything() {
        let index = index_from_names(&["a", "b", "c"]);
        let search = SearchIndex::build(&index);
        assert_eq!(search.search("").len(), 3);
    }

    #[test]
    fn no_match_returns_nothing() {
        let index = index_from_names(&["alpha", "beta"]);
        let search = SearchIndex::build(&index);
        assert!(search.search("zeta").is_empty());
    }

    #[test]
    fn handles_non_ascii_names() {
        let index = index_from_names(&["Çalışma Raporu.docx", "Ödeme.xlsx"]);
        let search = SearchIndex::build(&index);
        assert_eq!(
            names_of(&index, &search.search("raporu")),
            vec!["Çalışma Raporu.docx"]
        );
        assert_eq!(
            names_of(&index, &search.search("ödeme")),
            vec!["Ödeme.xlsx"]
        );
    }

    #[test]
    fn parallel_and_serial_paths_agree() {
        let names: Vec<String> = (0..PARALLEL_THRESHOLD + 5_000)
            .map(|i| format!("file_{i}.dat"))
            .collect();
        // index_from_names wants 'static strs, so build the index directly.
        let specs = names
            .iter()
            .enumerate()
            .map(|(i, name)| crate::mft::test_support::Spec {
                record: 16 + i as u32,
                parent: ROOT_RECORD,
                name: Box::leak(name.clone().into_boxed_str()),
                flags: 0,
                size: 0,
            })
            .collect();
        let index = index_from_specs(specs);
        let search = SearchIndex::build(&index);

        let parallel = search.search("file_1");
        let serial = search.search_range("file_1", 0, search.len());
        assert_eq!(parallel, serial);
        assert!(
            parallel.windows(2).all(|w| w[0] < w[1]),
            "hits must stay ordered"
        );
    }

    fn sample_tree() -> Index {
        index_from_specs(vec![
            dir(20, ROOT_RECORD, "Users"),
            dir(21, 20, "pc"),
            dir(22, 21, "Belgeler"),
            file(23, 22, "rapor.docx"),
            file(24, 21, "rapor.pdf"),
            dir(25, ROOT_RECORD, "Windows"),
            file(26, 25, "rapor.log"),
        ])
    }

    #[test]
    fn a_path_query_narrows_to_one_directory() {
        let index = sample_tree();
        let search = SearchIndex::build(&index);

        let all = Query {
            text: "rapor".into(),
            ..Default::default()
        };
        assert_eq!(search.run(&index, &all).len(), 3);

        let scoped = Query {
            text: r"belgeler\rapor".into(),
            ..Default::default()
        };
        let hits = search.run(&index, &scoped);
        assert_eq!(names_of(&index, &hits), vec!["rapor.docx"]);

        // Forward slashes work the same way.
        let with_slash = Query {
            text: "windows/rapor".into(),
            ..Default::default()
        };
        assert_eq!(
            names_of(&index, &search.run(&index, &with_slash)),
            vec!["rapor.log"]
        );
    }

    #[test]
    fn a_path_query_that_matches_nothing_returns_nothing() {
        let index = sample_tree();
        let search = SearchIndex::build(&index);
        let query = Query {
            text: r"program files\rapor".into(),
            ..Default::default()
        };
        assert!(search.run(&index, &query).is_empty());
    }

    #[test]
    fn sorts_by_name_size_and_path() {
        let index = index_from_specs(vec![
            crate::mft::test_support::Spec {
                record: 20,
                parent: ROOT_RECORD,
                name: "beta.txt",
                flags: 0,
                size: 300,
            },
            crate::mft::test_support::Spec {
                record: 21,
                parent: ROOT_RECORD,
                name: "Alpha.txt",
                flags: 0,
                size: 100,
            },
            crate::mft::test_support::Spec {
                record: 22,
                parent: ROOT_RECORD,
                name: "gamma.txt",
                flags: 0,
                size: 200,
            },
        ]);
        let search = SearchIndex::build(&index);

        let by_name = Query {
            text: ".txt".into(),
            sort_by: SortBy::Name,
            ..Default::default()
        };
        assert_eq!(
            names_of(&index, &search.run(&index, &by_name)),
            vec!["Alpha.txt", "beta.txt", "gamma.txt"]
        );

        let by_size = Query {
            text: ".txt".into(),
            sort_by: SortBy::Size,
            ..Default::default()
        };
        assert_eq!(
            names_of(&index, &search.run(&index, &by_size)),
            vec!["Alpha.txt", "gamma.txt", "beta.txt"]
        );

        let biggest = Query {
            text: ".txt".into(),
            sort_by: SortBy::Size,
            order: SortOrder::Descending,
            ..Default::default()
        };
        assert_eq!(
            names_of(&index, &search.run(&index, &biggest)),
            vec!["beta.txt", "gamma.txt", "Alpha.txt"]
        );

        let by_path = Query {
            text: ".txt".into(),
            sort_by: SortBy::Path,
            ..Default::default()
        };
        assert_eq!(
            names_of(&index, &search.run(&index, &by_path)),
            vec!["Alpha.txt", "beta.txt", "gamma.txt"]
        );
    }

    #[test]
    fn filters_by_kind_and_size() {
        let entry = |flags: u16, size: u64| crate::mft::test_support::bare_entry(flags, size, 0);

        let files = Filter {
            files_only: true,
            ..Default::default()
        };
        assert!(files.keeps(&entry(0, 0)));
        assert!(!files.keeps(&entry(crate::mft::IS_DIR, 0)));

        let dirs = Filter {
            dirs_only: true,
            ..Default::default()
        };
        assert!(dirs.keeps(&entry(crate::mft::IS_DIR, 0)));
        assert!(!dirs.keeps(&entry(0, 0)));

        let range = Filter {
            min_size: Some(1000),
            max_size: Some(2000),
            ..Default::default()
        };
        assert!(range.keeps(&entry(0, 1500)));
        assert!(!range.keeps(&entry(0, 999)));
        assert!(!range.keeps(&entry(0, 2001)));

        let visible = Filter {
            skip_hidden: true,
            ..Default::default()
        };
        assert!(visible.keeps(&entry(0, 0)));
        assert!(!visible.keeps(&entry(crate::mft::IS_HIDDEN, 0)));
        assert!(!visible.keeps(&entry(crate::mft::IS_SYSTEM, 0)));
    }

    #[test]
    fn filters_apply_to_query_results() {
        let index = sample_tree();
        let search = SearchIndex::build(&index);

        let dirs_only = Query {
            text: "".into(),
            filter: Filter {
                dirs_only: true,
                ..Default::default()
            },
            sort_by: SortBy::Name,
            ..Default::default()
        };
        assert_eq!(
            names_of(&index, &search.run(&index, &dirs_only)),
            vec!["Belgeler", "pc", "Users", "Windows"]
        );
    }
}
