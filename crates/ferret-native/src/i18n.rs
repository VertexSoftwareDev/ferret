//! Interface text.
//!
//! Only the chrome is translated. File names, paths, sizes and dates come from
//! the disk and are shown exactly as the filesystem has them — a Turkish
//! interface does not rename anyone's files.
//!
//! Failures arrive from `ferret-shell` as `code` or `code:detail`, never as a
//! finished sentence, so a message raised while the window was in Turkish still
//! reads correctly after switching to English. [`Lang::error`] is where a code
//! becomes words.
//!
//! The plain strings sit in a struct and the ones that take an argument are
//! methods. Two tables of format strings would be shorter to write and much
//! easier to get subtly wrong — a missing `{}` in one language only shows up
//! when somebody switches to it.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Lang {
    Tr,
    En,
}

impl Default for Lang {
    /// Turkish when Windows is Turkish, English otherwise.
    fn default() -> Self {
        match system_language().as_deref() {
            Some(tag) if tag.to_ascii_lowercase().starts_with("tr") => Lang::Tr,
            _ => Lang::En,
        }
    }
}

/// Every string that needs no argument.
pub struct Strings {
    pub search_placeholder: &'static str,
    pub search_tooltip: &'static str,
    pub drive_tooltip: &'static str,
    pub language_tooltip: &'static str,
    pub rescan_tooltip: &'static str,
    pub theme_tooltip: &'static str,

    pub files_only: &'static str,
    pub dirs_only: &'static str,
    pub skip_hidden: &'static str,
    pub at_least: &'static str,
    pub any_size: &'static str,

    pub col_name: &'static str,
    pub col_path: &'static str,
    pub col_kind: &'static str,
    pub col_size: &'static str,
    pub col_date: &'static str,
    pub folder: &'static str,

    pub menu_open: &'static str,
    pub menu_reveal: &'static str,
    pub menu_copy: &'static str,
    pub menu_search_here: &'static str,

    pub preparing: &'static str,
    pub scanning_detail: &'static str,
    pub scan_failed: &'static str,
    pub retry: &'static str,

    pub elevation_title: &'static str,
    pub elevation_detail: &'static str,
    pub elevation_action: &'static str,

    pub no_volume_title: &'static str,
    pub no_volume_detail: &'static str,

    pub empty_hint: &'static str,

    pub sort_skipped: &'static str,
    pub index_stale: &'static str,
    pub copied: &'static str,
}

const TR: Strings = Strings {
    search_placeholder: "Dosya adı ara…   (yol için:  belgeler/rapor)",
    search_tooltip: "Arama  ·  Ctrl+F",
    drive_tooltip: "Sürücü",
    language_tooltip: "Dil",
    rescan_tooltip: "Yeniden tara (F5)",
    theme_tooltip: "Tema değiştir",

    files_only: "Yalnızca dosya",
    dirs_only: "Yalnızca klasör",
    skip_hidden: "Gizlileri atla",
    at_least: "En az",
    any_size: "—",

    col_name: "Ad",
    col_path: "Konum",
    col_kind: "Tür",
    col_size: "Boyut",
    col_date: "Değiştirilme",
    folder: "Klasör",

    menu_open: "Aç",
    menu_reveal: "Klasörde göster",
    menu_copy: "Yolu kopyala",
    menu_search_here: "Bu klasörde ara",

    preparing: "Hazırlanıyor…",
    scanning_detail: "Ana dosya tablosu okunuyor. Bu, diskin tamamı için birkaç saniye sürer.",
    scan_failed: "Taranamadı",
    retry: "Tekrar dene",

    elevation_title: "Yönetici izni gerekiyor",
    elevation_detail: concat!(
        "Ferret, diski doğrudan okuyarak indeksliyor; Windows bunun için yönetici izni istiyor. ",
        "Diske yalnızca okuma yapılır, hiçbir şey yazılmaz."
    ),
    elevation_action: "Yönetici olarak yeniden başlat",

    no_volume_title: "NTFS sürücüsü bulunamadı",
    no_volume_detail: "Ferret yalnızca NTFS birimlerini okuyabilir.",

    // Ferret accepts either slash in a path query, and the forward one keeps
    // this string free of escapes.
    empty_hint: concat!(
        "Her kelime eşleşmeli. Joker karakter için *.pdf, klasör içinde aramak için ",
        "belgeler/rapor, boşluklu ad için \"yıllık rapor\" dene."
    ),

    sort_skipped: "Sonuç kümesi çok büyük olduğu için sıralama uygulanmadı.",
    index_stale: "Disk çok değişti; güncel sonuçlar için F5 ile yeniden tara.",
    copied: "Yol kopyalandı.",
};

const EN: Strings = Strings {
    search_placeholder: "Search file names…   (for a path:  documents/report)",
    search_tooltip: "Search  ·  Ctrl+F",
    drive_tooltip: "Drive",
    language_tooltip: "Language",
    rescan_tooltip: "Re-index (F5)",
    theme_tooltip: "Switch theme",

    files_only: "Files only",
    dirs_only: "Folders only",
    skip_hidden: "Skip hidden",
    at_least: "At least",
    any_size: "—",

    col_name: "Name",
    col_path: "Location",
    col_kind: "Type",
    col_size: "Size",
    col_date: "Modified",
    folder: "Folder",

    menu_open: "Open",
    menu_reveal: "Show in folder",
    menu_copy: "Copy path",
    menu_search_here: "Search in this folder",

    preparing: "Getting ready…",
    scanning_detail: "Reading the master file table. A whole disk takes a few seconds.",
    scan_failed: "Could not index",
    retry: "Try again",

    elevation_title: "Administrator rights needed",
    elevation_detail: concat!(
        "Ferret indexes by reading the disk directly, and Windows requires administrator ",
        "rights for that. The disk is only ever read, never written to."
    ),
    elevation_action: "Restart as administrator",

    no_volume_title: "No NTFS drive found",
    no_volume_detail: "Ferret can only read NTFS volumes.",

    empty_hint: concat!(
        "Every word has to match. Try *.pdf for a wildcard, documents/report to search ",
        "inside a folder, or \"annual report\" for a name with a space."
    ),

    sort_skipped: "Too many results to sort, so the order is as found.",
    index_stale: "The disk has changed a lot; press F5 to re-index.",
    copied: "Path copied.",
};

impl Lang {
    pub fn strings(self) -> &'static Strings {
        match self {
            Lang::Tr => &TR,
            Lang::En => &EN,
        }
    }

    /// What the language is called in the language picker.
    pub fn label(self) -> &'static str {
        match self {
            Lang::Tr => "TR",
            Lang::En => "EN",
        }
    }

    pub fn scanning(self, letter: &str) -> String {
        match self {
            Lang::Tr => format!("{letter}: taranıyor"),
            Lang::En => format!("Indexing {letter}:"),
        }
    }

    pub fn empty_title(self, query: &str) -> String {
        match self {
            Lang::Tr => format!("“{query}” için sonuç yok"),
            Lang::En => format!("No results for “{query}”"),
        }
    }

    pub fn results(self, count: u64) -> String {
        let count = self.number(count);
        match self {
            Lang::Tr => format!("{count} sonuç"),
            Lang::En => format!("{count} results"),
        }
    }

    pub fn total_size(self, size: &str) -> String {
        match self {
            Lang::Tr => format!("toplam {size}"),
            Lang::En => format!("{size} in total"),
        }
    }

    pub fn timing(self, ms: f64) -> String {
        format!("{ms:.1} ms")
    }

    /// The status bar's right half: what was indexed, how long it took, and
    /// that the index is being kept up to date.
    pub fn volume_line(self, letter: &str, files: u64, dirs: u64, seconds: f64, memory: &str) -> String {
        let files = self.number(files);
        let dirs = self.number(dirs);
        match self {
            Lang::Tr => format!(
                "{letter}:  {files} dosya · {dirs} klasör · {seconds:.1} sn'de tarandı · {memory}  ·  canlı"
            ),
            Lang::En => format!(
                "{letter}:  {files} files · {dirs} folders · indexed in {seconds:.1} s · {memory}  ·  live"
            ),
        }
    }

    pub fn title_volume(self, letter: &str, files: u64) -> String {
        let files = self.number(files);
        match self {
            Lang::Tr => format!("Ferret — {letter}: {files} dosya"),
            Lang::En => format!("Ferret — {letter}: {files} files"),
        }
    }

    pub fn title_results(self, count: u64) -> String {
        let count = self.number(count);
        match self {
            Lang::Tr => format!("Ferret — {count} sonuç"),
            Lang::En => format!("Ferret — {count} results"),
        }
    }

    /// Turn a `code` or `code:detail` failure into a sentence.
    ///
    /// Anything unrecognised is shown as it arrived: an operating system message
    /// is more use than "something went wrong".
    pub fn error(self, raw: &str) -> String {
        let (code, detail) = match raw.split_once(':') {
            Some((code, detail)) => (code, detail),
            None => (raw, ""),
        };

        let phrased = match (self, code) {
            (Lang::Tr, "no_drive_letter") => "Sürücü seçilmedi.".to_string(),
            (Lang::En, "no_drive_letter") => "No drive selected.".to_string(),
            (Lang::Tr, "needs_elevation") => {
                "Erişim reddedildi. Yönetici izni gerekiyor.".to_string()
            }
            (Lang::En, "needs_elevation") => {
                "Access denied. Administrator rights are needed.".to_string()
            }
            (Lang::Tr, "drive_not_found") => "Sürücü bulunamadı.".to_string(),
            (Lang::En, "drive_not_found") => "Drive not found.".to_string(),
            (Lang::Tr, "not_indexed") => format!("{detail}: henüz taranmadı."),
            (Lang::En, "not_indexed") => format!("{detail}: not indexed yet."),
            (Lang::Tr, "scan_failed") => format!("Taranamadı: {detail}"),
            (Lang::En, "scan_failed") => format!("Could not index: {detail}"),
            (Lang::Tr, "open_failed") => format!("Dosya açılamadı: {detail}"),
            (Lang::En, "open_failed") => format!("Could not open the file: {detail}"),
            (Lang::Tr, "reveal_failed") => format!("Klasör açılamadı: {detail}"),
            (Lang::En, "reveal_failed") => format!("Could not open the folder: {detail}"),
            (Lang::Tr, "restart_failed") => format!("Yeniden başlatılamadı: {detail}"),
            (Lang::En, "restart_failed") => format!("Could not restart: {detail}"),
            _ => raw.to_string(),
        };
        phrased
    }

    /// Group a count the way the language writes numbers: 1.559.903 in Turkish,
    /// 1,559,903 in English.
    ///
    /// Done by hand because it is the only localised number in the app, and a
    /// formatting crate would be a megabyte of locale data for one separator.
    pub fn number(self, value: u64) -> String {
        let separator = match self {
            Lang::Tr => '.',
            Lang::En => ',',
        };
        let digits = value.to_string();
        let mut out = String::with_capacity(digits.len() + digits.len() / 3);
        for (position, digit) in digits.chars().enumerate() {
            if position > 0 && (digits.len() - position).is_multiple_of(3) {
                out.push(separator);
            }
            out.push(digit);
        }
        out
    }
}

/// The language Windows is running in, as a BCP-47 tag like `tr-TR`.
///
/// Read from the environment rather than through `GetUserDefaultLocaleName`,
/// which would mean a Windows API dependency for one string. When nothing says
/// otherwise the caller falls back to English, which is the safer default for a
/// tool whose error messages people paste into search engines.
fn system_language() -> Option<String> {
    for key in ["LANG", "LC_ALL", "LANGUAGE"] {
        if let Ok(value) = std::env::var(key) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_grouped_the_way_each_language_writes_them() {
        assert_eq!(Lang::Tr.number(1_559_903), "1.559.903");
        assert_eq!(Lang::En.number(1_559_903), "1,559,903");
        // Boundaries: nothing to group, and exactly one group.
        assert_eq!(Lang::En.number(0), "0");
        assert_eq!(Lang::En.number(999), "999");
        assert_eq!(Lang::En.number(1_000), "1,000");
        assert_eq!(Lang::Tr.number(1_000_000), "1.000.000");
    }

    #[test]
    fn a_bare_code_becomes_a_sentence() {
        assert_eq!(
            Lang::En.error("needs_elevation"),
            "Access denied. Administrator rights are needed."
        );
        assert!(Lang::Tr.error("needs_elevation").contains("Yönetici"));
    }

    #[test]
    fn a_code_with_a_detail_keeps_the_detail() {
        assert_eq!(Lang::En.error("not_indexed:D"), "D: not indexed yet.");
        let scan = Lang::En.error("scan_failed:the disk is on fire");
        assert_eq!(scan, "Could not index: the disk is on fire");
    }

    #[test]
    fn an_unknown_code_is_shown_as_it_arrived() {
        // Better a raw operating system message than "something went wrong".
        assert_eq!(Lang::En.error("Os { code: 5 }"), "Os { code: 5 }");
        assert_eq!(Lang::Tr.error("whatever:detail"), "whatever:detail");
    }

    /// The two tables have to stay the same shape, or switching language would
    /// blank part of the window.
    #[test]
    fn neither_language_has_an_empty_string() {
        for lang in [Lang::Tr, Lang::En] {
            let s = lang.strings();
            for (field, value) in [
                ("search_placeholder", s.search_placeholder),
                ("search_tooltip", s.search_tooltip),
                ("drive_tooltip", s.drive_tooltip),
                ("language_tooltip", s.language_tooltip),
                ("rescan_tooltip", s.rescan_tooltip),
                ("theme_tooltip", s.theme_tooltip),
                ("files_only", s.files_only),
                ("dirs_only", s.dirs_only),
                ("skip_hidden", s.skip_hidden),
                ("at_least", s.at_least),
                ("col_name", s.col_name),
                ("col_path", s.col_path),
                ("col_kind", s.col_kind),
                ("col_size", s.col_size),
                ("col_date", s.col_date),
                ("folder", s.folder),
                ("menu_open", s.menu_open),
                ("menu_reveal", s.menu_reveal),
                ("menu_copy", s.menu_copy),
                ("menu_search_here", s.menu_search_here),
                ("preparing", s.preparing),
                ("scanning_detail", s.scanning_detail),
                ("scan_failed", s.scan_failed),
                ("retry", s.retry),
                ("elevation_title", s.elevation_title),
                ("elevation_detail", s.elevation_detail),
                ("elevation_action", s.elevation_action),
                ("no_volume_title", s.no_volume_title),
                ("no_volume_detail", s.no_volume_detail),
                ("empty_hint", s.empty_hint),
                ("sort_skipped", s.sort_skipped),
                ("index_stale", s.index_stale),
                ("copied", s.copied),
            ] {
                assert!(!value.is_empty(), "{lang:?} has no {field}");
            }
        }
    }

    #[test]
    fn the_parameterised_strings_actually_carry_their_argument() {
        for lang in [Lang::Tr, Lang::En] {
            assert!(lang.scanning("C").contains('C'));
            assert!(lang.empty_title("rapor").contains("rapor"));
            assert!(lang.total_size("1.4 GB").contains("1.4 GB"));
            assert!(lang.title_volume("C", 12).contains("12"));
            assert!(lang.title_results(7).contains('7'));

            let line = lang.volume_line("C", 1_000, 20, 6.5, "163 MB");
            assert!(line.contains("163 MB"), "{line}");
            assert!(line.contains("6.5"), "{line}");
            assert!(line.starts_with("C:"), "{line}");
        }
        assert_eq!(Lang::En.timing(9.04), "9.0 ms");
    }
}
