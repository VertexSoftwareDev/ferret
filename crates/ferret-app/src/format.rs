//! Display formatting shared by the commands.
//!
//! Formatting happens in Rust rather than in the web view because a result page
//! is at most a screenful of rows: doing it here keeps the front end free of
//! date and unit logic, and keeps the numbers identical to the CLI's.

/// Human-readable byte size, e.g. `1.4 GB`. Directories have no size of their
/// own, so they get an empty string rather than a misleading `0 B`.
pub fn size(bytes: u64, is_dir: bool) -> String {
    if is_dir {
        return String::new();
    }
    ferret_core::human_size(bytes)
}

/// Render a Windows FILETIME as `YYYY-MM-DD HH:MM`, in UTC.
///
/// Implemented by hand rather than pulling in a date crate: this is the only
/// date the app ever shows, and the civil-from-days algorithm is a dozen lines.
pub fn timestamp(filetime: u64) -> String {
    let Some(unix) = ferret_core::filetime_to_unix(filetime) else {
        return String::new();
    };
    if unix < 0 {
        return String::new();
    }

    let days = unix.div_euclid(86_400);
    let seconds_of_day = unix.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Howard Hinnant's `civil_from_days`: turn a day count since the Unix epoch
/// into a calendar date, leap years and all.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    // Shifted months run March=0 … February=11; convert back to January=1.
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;

    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// File extension in upper case, for the type column. Directories report
/// nothing; a name with no dot reports nothing.
pub fn kind(name: &str, is_dir: bool) -> String {
    if is_dir {
        return "Klasör".to_string();
    }
    match name.rsplit_once('.') {
        // A leading dot means a dotfile, not an extension.
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 8 => {
            ext.to_uppercase()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes() {
        assert_eq!(size(0, true), "");
        assert_eq!(size(512, false), "512 B");
        assert_eq!(size(1024, false), "1.0 KB");
    }

    #[test]
    fn formats_known_timestamps() {
        // 1970-01-01T00:00:00Z
        assert_eq!(timestamp(116_444_736_000_000_000), "1970-01-01 00:00");
        // 2001-01-01T00:00:00Z
        assert_eq!(timestamp(126_227_808_000_000_000), "2001-01-01 00:00");
        // 2024-02-29T13:45:00Z — a leap day, which is where naive date code breaks.
        assert_eq!(timestamp(133_536_879_000_000_000), "2024-02-29 13:45");
        // 2023-03-01T00:00:00Z — the day after February in a non-leap year.
        assert_eq!(timestamp(133_221_024_000_000_000), "2023-03-01 00:00");
        // 2000-02-29T23:59:00Z — a century leap year, the other classic trap.
        assert_eq!(timestamp(125_963_423_400_000_000), "2000-02-29 23:59");
        // Never set.
        assert_eq!(timestamp(0), "");
    }

    #[test]
    fn derives_the_kind_column() {
        assert_eq!(kind("notes.txt", false), "TXT");
        assert_eq!(kind("archive.tar.gz", false), "GZ");
        assert_eq!(kind("Makefile", false), "");
        assert_eq!(kind(".gitignore", false), "");
        assert_eq!(kind("Belgeler", true), "Klasör");
    }
}
