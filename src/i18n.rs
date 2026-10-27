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
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut number = value as f64;
    let mut unit = 0;
    while number >= 1024.0 && unit + 1 < units.len() {
        number /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", integer(language, value))
    } else {
        let mut number = format!("{number:.1}");
        if matches!(TAGS[language.min(6)], "ru" | "es" | "de" | "fr" | "pt-BR") {
            number = number.replace('.', ",");
        }
        format!("{number} {}", units[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
