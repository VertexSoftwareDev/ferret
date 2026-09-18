//! The handful of choices worth surviving a restart.
//!
//! Deliberately not the query itself: reopening Ferret to yesterday's search
//! would be a surprise, and the search box is where the eye goes first anyway.
//!
//! Stored through eframe, which puts it under the user's own application data —
//! the native counterpart of the web view's `localStorage`. Every field carries
//! `serde(default)`, so a file written by an older or newer build still loads
//! and only the fields it does not know about fall back.

use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::theme::Theme;

/// Where eframe keeps this between runs.
pub const KEY: &str = "ferret-prefs";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Prefs {
    pub lang: Lang,
    pub theme: Theme,
    /// Drive letter as a single-character string, e.g. `C`.
    pub drive: String,
    /// One of `name`, `path`, `size`, `modified`, or empty for scan order.
    pub sort_by: String,
    pub descending: bool,
    pub files_only: bool,
    pub dirs_only: bool,
    pub skip_hidden: bool,
    pub min_size: Option<u64>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            lang: Lang::default(),
            theme: Theme::default(),
            drive: String::new(),
            sort_by: String::new(),
            descending: false,
            files_only: false,
            // On by default: a first search full of AppData and $Recycle.Bin is
            // nobody's idea of a useful result.
            skip_hidden: true,
            dirs_only: false,
            min_size: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_files_are_skipped_until_asked_for() {
        assert!(Prefs::default().skip_hidden);
        assert!(!Prefs::default().files_only);
        assert_eq!(Prefs::default().min_size, None);
    }

    /// The point of `serde(default)`: a file from a build that had fewer fields
    /// still loads, rather than resetting every choice the user made.
    #[test]
    fn a_partial_file_keeps_what_it_says_and_defaults_the_rest() {
        let json = r#"{"drive":"D","sort_by":"size"}"#;
        let loaded: Prefs = serde_json::from_str(json).expect("partial prefs load");

        assert_eq!(loaded.drive, "D");
        // Unknown-to-this-file fields fall back rather than blanking.
        assert!(loaded.skip_hidden);
        assert_eq!(loaded.theme, Theme::Dark);
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        let loaded: Prefs = serde_json::from_str("{}").expect("empty prefs load");
        assert_eq!(loaded, Prefs::default());
    }
}
