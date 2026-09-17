//! Instant substring search over an [`Index`].
//!
//! The naive version — lowercase every name on every keystroke — costs about
//! 100 ms on a 1.8 M file volume, because it allocates a fresh string per file
//! per query. That is far too slow to feel live while typing.
//!
//! So the lowercase form is computed **once**, at build time, into a single
//! contiguous arena with `\n` between names. A query then becomes one linear
//! scan of ~40 MB of bytes, which the two-way matcher behind `str::find` chews
//! through at gigabytes per second, split across all cores. NTFS forbids
//! control characters in filenames, so `\n` is a separator that can never occur
//! inside a name.

use std::thread;

use crate::mft::{Entry, Index};

/// Byte that separates names in the arena; illegal inside an NTFS filename.
const SEPARATOR: u8 = b'\n';

/// Below this many entries the thread hand-off costs more than it saves.
const PARALLEL_THRESHOLD: usize = 50_000;

/// A prepared, lowercase copy of every name in an index.
///
/// Built once per scan; queried as often as the user types.
pub struct SearchIndex {
    /// All names, lowercased, joined by [`SEPARATOR`].
    haystack: String,
    /// `starts[i]` is where entry `i`'s name begins in `haystack`.
    starts: Vec<u32>,
}

/// One hit: the position of the entry inside [`Index::entries`].
pub type Hit = u32;

impl SearchIndex {
    /// Prepare the lowercase arena for an index.
    pub fn build(index: &Index) -> SearchIndex {
        let entries = index.entries();
        // Names average out around 20 bytes; over-reserving beats regrowing a
        // 40 MB string a dozen times.
        let mut haystack = String::with_capacity(entries.len() * 24);
        let mut starts = Vec::with_capacity(entries.len());

        for entry in entries {
            starts.push(haystack.len() as u32);
            push_lowercase(&mut haystack, &entry.name);
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
            thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
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

/// Append `text` lowercased.
///
/// ASCII is the overwhelmingly common case and is folded in place without
/// allocating; anything else falls back to Unicode-aware lowercasing.
fn push_lowercase(out: &mut String, text: &str) {
    if text.is_ascii() {
        // SAFETY-free path: ASCII lowercasing maps each byte to another ASCII
        // byte, so the result stays valid UTF-8.
        out.extend(text.chars().map(|c| c.to_ascii_lowercase()));
    } else {
        out.extend(text.chars().flat_map(char::to_lowercase));
    }
}

/// Filters applied on top of a text match.
#[derive(Debug, Default, Clone, Copy)]
pub struct Filter {
    pub files_only: bool,
    pub dirs_only: bool,
    /// Keep entries at least this large, in bytes.
    pub min_size: Option<u64>,
}

impl Filter {
    pub fn keeps(&self, entry: &Entry) -> bool {
        if self.files_only && entry.is_dir {
            return false;
        }
        if self.dirs_only && !entry.is_dir {
            return false;
        }
        if let Some(min) = self.min_size {
            if entry.size < min {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mft::test_support::index_from_names;

    fn names_of<'a>(index: &'a Index, hits: &[Hit]) -> Vec<&'a str> {
        hits.iter()
            .map(|h| index.entries()[*h as usize].name.as_ref())
            .collect()
    }

    #[test]
    fn finds_substrings_case_insensitively() {
        let index = index_from_names(&["Rapor.pdf", "notlar.txt", "RAPORLAR", "foto.jpg"]);
        let search = SearchIndex::build(&index);

        assert_eq!(names_of(&index, &search.search("rapor")), vec!["Rapor.pdf", "RAPORLAR"]);
        assert_eq!(names_of(&index, &search.search("RAPOR")), vec!["Rapor.pdf", "RAPORLAR"]);
        assert_eq!(names_of(&index, &search.search(".txt")), vec!["notlar.txt"]);
    }

    #[test]
    fn reports_each_name_once_even_with_repeats() {
        let index = index_from_names(&["aaa.txt", "b.txt"]);
        let search = SearchIndex::build(&index);
        // "a" occurs three times in the first name.
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
        assert_eq!(names_of(&index, &search.search("raporu")), vec!["Çalışma Raporu.docx"]);
        assert_eq!(names_of(&index, &search.search("ödeme")), vec!["Ödeme.xlsx"]);
    }

    #[test]
    fn parallel_and_serial_paths_agree() {
        // Enough entries to cross the threshold and actually fan out.
        let mut names: Vec<String> = (0..PARALLEL_THRESHOLD + 5_000)
            .map(|i| format!("file_{i}.dat"))
            .collect();
        names.push("needle_here.txt".to_string());
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();

        let index = index_from_names(&refs);
        let search = SearchIndex::build(&index);

        assert_eq!(names_of(&index, &search.search("needle_here")), vec!["needle_here.txt"]);
        // "file_1" matches 1, 1x, 1xx … across every worker's range.
        let parallel = search.search("file_1");
        let serial = search.search_range("file_1", 0, search.len());
        assert_eq!(parallel, serial);
        assert!(parallel.windows(2).all(|w| w[0] < w[1]), "hits must stay ordered");
    }

    #[test]
    fn filters_by_kind_and_size() {
        let entry = |is_dir, size| Entry {
            record: 20,
            parent: 5,
            name: "x".into(),
            is_dir,
            size,
        };

        let files = Filter { files_only: true, ..Default::default() };
        assert!(files.keeps(&entry(false, 0)));
        assert!(!files.keeps(&entry(true, 0)));

        let big = Filter { min_size: Some(1000), ..Default::default() };
        assert!(big.keeps(&entry(false, 2000)));
        assert!(!big.keeps(&entry(false, 999)));
    }
}
