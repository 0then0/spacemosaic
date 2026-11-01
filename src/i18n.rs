//! Bundled translations and locale-aware display formatting.
use std::{collections::BTreeMap, sync::OnceLock};

pub const TAGS: [&str; 7] = ["en", "ru", "es", "de", "fr", "ja", "pt-BR"];

/// Format measured bytes without presenting missing metadata as an empty object.
pub fn measure(language: usize, size: crate::scan::Measure) -> String {
    if size.overflow {
        return text(language, "overflow").into();
    }
    if size.known == 0 && size.unknown > 0 {
        return format!(
            "{} ({})",
            text(language, "unknown"),
            integer(language, size.unknown)
        );
    }
    let mut value = bytes(language, size.known);
    if size.unknown > 0 {
        value.push_str(&format!(
            " + {} {}",
            integer(language, size.unknown),
            text(language, "unknown")
        ));
    }
    value
}

fn catalogs() -> &'static [BTreeMap<String, String>; 7] {
    static CATALOGS: OnceLock<[BTreeMap<String, String>; 7]> = OnceLock::new();
    CATALOGS.get_or_init(|| {
        [
            include_str!("../assets/i18n/en.json"),
            include_str!("../assets/i18n/ru.json"),
            include_str!("../assets/i18n/es.json"),
            include_str!("../assets/i18n/de.json"),
            include_str!("../assets/i18n/fr.json"),
            include_str!("../assets/i18n/ja.json"),
            include_str!("../assets/i18n/pt-BR.json"),
        ]
        .map(|source| serde_json::from_str(source).expect("validated bundled catalog"))
    })
}

pub fn system_language() -> usize {
    fastframe_i18n::detect(|tag| {
        TAGS.iter().position(|candidate| {
            fastframe_i18n::LanguageTag::parse(candidate)
                .is_some_and(|c| c.language == tag.language)
        })
    })
    .unwrap_or(0)
}

pub fn text(language: usize, key: &str) -> &str {
    catalogs()[language.min(6)]
        .get(key)
        .map(String::as_str)
        .unwrap_or(key)
}

/// Deterministic localized grouping; exported machine numbers are never formatted this way.
pub fn integer(language: usize, value: u64) -> String {
    let separator = match TAGS[language.min(6)] {
        "ru" | "fr" => '\u{202f}',
        "de" | "es" | "pt-BR" => '.',
        _ => ',',
    };
    let digits = value.to_string();
    let mut output = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(separator);
        }
        output.push(ch);
    }
    output
}
pub fn bytes(language: usize, value: u64) -> String {
    bytes_wide(language, u128::from(value))
}
/// Sums of many u64 sizes can exceed u64; only the display is rounded.
pub fn bytes_wide(language: usize, value: u128) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut number = value as f64;
    let mut unit = 0;
    while number >= 1024.0 && unit + 1 < units.len() {
        number /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", integer(language, value as u64))
    } else {
        let mut number = format!("{number:.1}");
        if matches!(TAGS[language.min(6)], "ru" | "es" | "de" | "fr" | "pt-BR") {
            number = number.replace('.', ",");
        }
        format!("{number} {}", units[unit])
    }
}

/// A change in bytes with an explicit sign, so direction never depends on color.
pub fn signed_bytes(language: usize, value: i128) -> String {
    let text = bytes_wide(language, value.unsigned_abs());
    match value.signum() {
        1 => format!("+{text}"),
        -1 => format!("\u{2212}{text}"),
        _ => text,
    }
}

/// UTC date and time of a recorded scan, independent of display language.
pub fn timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Civil-from-days (proleptic Gregorian), after H. Hinnant.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_and_signed_sizes_are_language_independent_and_exact() {
        assert_eq!(timestamp(0), "1970-01-01 00:00 UTC");
        assert_eq!(timestamp(1_790_000_000), "2026-09-21 14:13 UTC");
        assert_eq!(timestamp(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(signed_bytes(0, 0), "0 B");
        assert_eq!(signed_bytes(0, 50 * 1024 * 1024), "+50.0 MiB");
        assert_eq!(signed_bytes(3, -(400 * 1024 * 1024)), "\u{2212}400,0 MiB");
        assert_eq!(signed_bytes(0, -(u64::MAX as i128)), "\u{2212}16.0 EiB");
        assert!(signed_bytes(0, 2 * u64::MAX as i128).starts_with('+'));
    }
    #[test]
    fn all_seven_catalogs_have_complete_nonempty_keys() {
        let expected: Vec<_> = catalogs()[0].keys().collect();
        for catalog in catalogs() {
            assert_eq!(catalog.keys().collect::<Vec<_>>(), expected);
            assert!(catalog.values().all(|value| !value.trim().is_empty()));
        }
    }
}

pub fn counted(language: usize, count: u64, noun: &str) -> String {
    let form = match TAGS[language.min(6)] {
        "ru" if count % 10 == 1 && count % 100 != 11 => "one",
        "ru" if (2..=4).contains(&(count % 10)) && !(12..=14).contains(&(count % 100)) => "few",
        "ru" => "many",
        "ja" => "other",
        "fr" if count <= 1 => "one",
        _ if count == 1 => "one",
        _ => "other",
    };
    format!(
        "{} {}",
        integer(language, count),
        text(language, &format!("{noun}_{form}"))
    )
}

#[cfg(test)]
mod formatting_tests {
    use super::*;
    #[test]
    fn display_grouping_and_plural_forms_do_not_change_machine_numbers() {
        assert_eq!(integer(0, 1234567), "1,234,567");
        assert_eq!(integer(3, 1234567), "1.234.567");
        assert_eq!(counted(0, 1, "file"), "1 file");
        assert_eq!(counted(0, 2, "file"), "2 files");
        assert_eq!(counted(1, 1, "file"), "1 файл");
        assert_eq!(counted(1, 2, "file"), "2 файла");
        assert_eq!(counted(1, 11, "file"), "11 файлов");
        assert_eq!(counted(1, 21, "file"), "21 файл");
        assert_eq!(bytes(3, 1536), "1,5 KiB");
    }
}
