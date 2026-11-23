use std::{cmp::Ordering, ffi::OsStr, fmt::Write, path::Path};

use serde::{Deserialize, Serialize};

pub(crate) const MAX_EXCLUSION_PATH_BYTES: usize = 32 * 1024;

/// A normalized rule and whether the scan encountered a directory it skipped.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FolderExclusion {
    pub path: String,
    pub skipped: bool,
}

/// Validated root-relative paths for a local scan, in component order.
#[derive(Clone, Debug, Default)]
pub struct Exclusions(Vec<String>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidExclusionPath;

impl Exclusions {
    pub fn paths(&self) -> &[String] {
        &self.0
    }

    pub fn add(&mut self, input: &str) -> Result<(), InvalidExclusionPath> {
        let path = normalize(input, cfg!(windows)).ok_or(InvalidExclusionPath)?;
        let index = self
            .0
            .partition_point(|other| path_order(other, &path).is_lt());
        if self.0.get(index) == Some(&path) || index > 0 && covers(&self.0[index - 1], &path) {
            return Ok(());
        }
        let covered = self.0[index..]
            .iter()
            .take_while(|other| covers(&path, other))
            .count();
        self.0.splice(index..index + covered, [path]);
        Ok(())
    }

    pub fn remove(&mut self, index: usize) {
        self.0.remove(index);
    }

    pub(super) fn into_recorded(self) -> Vec<FolderExclusion> {
        self.0
            .into_iter()
            .map(|path| FolderExclusion {
                path,
                skipped: false,
            })
            .collect()
    }
}

pub(super) fn record_skip(rules: &mut [FolderExclusion], root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    if let Ok(index) = rules.binary_search_by(|rule| {
        rule.path
            .split('/')
            .map(Some)
            .cmp(relative.iter().map(OsStr::to_str))
    }) {
        rules[index].skipped = true;
        true
    } else {
        false
    }
}

fn normalize(input: &str, windows: bool) -> Option<String> {
    let separator = |ch| ch == '/' || (windows && ch == '\\');
    if input.len() > MAX_EXCLUSION_PATH_BYTES || input.starts_with(separator) {
        return None;
    }
    let mut parts = Vec::new();
    for part in input.split(separator) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part if valid_component(part, windows) => parts.push(part),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn valid_component(part: &str, windows: bool) -> bool {
    if part.is_empty() || matches!(part, "." | "..") || part.contains('\0') {
        return false;
    }
    if !windows {
        return true;
    }
    if part.ends_with([' ', '.']) || part.chars().any(|ch| ch < ' ' || "<>:\"\\|?*".contains(ch)) {
        return false;
    }
    let stem = part.split('.').next().unwrap_or(part).trim_end_matches(' ');
    if ["CON", "PRN", "AUX", "NUL"]
        .iter()
        .any(|name| stem.eq_ignore_ascii_case(name))
    {
        return false;
    }
    if let Some((prefix, suffix)) = stem.split_at_checked(3) {
        return !(prefix.eq_ignore_ascii_case("COM") || prefix.eq_ignore_ascii_case("LPT"))
            || !matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            );
    }
    true
}

fn path_order(a: &str, b: &str) -> Ordering {
    a.split('/').cmp(b.split('/'))
}

fn covers(parent: &str, path: &str) -> bool {
    path == parent
        || path
            .strip_prefix(parent)
            .is_some_and(|tail| tail.starts_with('/'))
}

pub(crate) fn normalized_rules(rules: &[FolderExclusion], windows: bool) -> bool {
    rules.iter().all(|rule| {
        rule.path.len() <= MAX_EXCLUSION_PATH_BYTES
            && rule
                .path
                .split('/')
                .all(|part| valid_component(part, windows))
    }) && rules.windows(2).all(|pair| {
        let (a, b) = (&pair[0].path, &pair[1].path);
        path_order(a, b).is_lt() && !covers(a, b)
    })
}

/// Exact native keys for the root-relative rules in a comparison or import.
/// Windows accepts both separators in recorded paths. Name spelling is kept.
pub(crate) struct NativeExclusions {
    windows: bool,
    keys: Vec<(String, bool)>,
}

impl NativeExclusions {
    pub(crate) fn estimated_bytes<'a>(rules: impl Iterator<Item = &'a FolderExclusion>) -> usize {
        rules.fold(0, |sum, rule| {
            sum.saturating_add(size_of::<(String, bool)>() + 16)
                .saturating_add(rule.path.len().saturating_mul(4))
        })
    }

    pub(crate) fn new<'a>(windows: bool, rules: impl Iterator<Item = &'a FolderExclusion>) -> Self {
        let mut keys = Vec::with_capacity(rules.size_hint().0);
        for rule in rules {
            let mut key = String::with_capacity(rule.path.len() * if windows { 4 } else { 2 });
            if windows {
                for unit in rule.path.encode_utf16() {
                    let unit = if unit == u16::from(b'/') {
                        u16::from(b'\\')
                    } else {
                        unit
                    };
                    let _ = write!(key, "{unit:04x}");
                }
            } else {
                for byte in rule.path.bytes() {
                    let _ = write!(key, "{byte:02x}");
                }
            }
            keys.push((key, rule.skipped));
        }
        keys.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        keys.dedup_by(|later, first| {
            if later.0 != first.0 {
                return false;
            }
            first.1 |= later.1;
            true
        });
        Self { windows, keys }
    }

    fn is_separator(&self, unit: &[u8]) -> bool {
        if self.windows {
            matches!(unit, b"005c" | b"002f")
        } else {
            unit == b"2f"
        }
    }

    fn width(&self) -> usize {
        if self.windows { 4 } else { 2 }
    }

    fn relative<'a>(&self, root: &str, path: &'a str) -> Option<&'a str> {
        let tail = path.strip_prefix(root)?;
        let width = self.width();
        let last = root.as_bytes().rchunks_exact(width).next()?;
        // PathBuf joins `C:` directly to a child name on Windows.
        let bare_drive =
            self.windows && root.len() == "windows-utf16:".len() + 8 && last == b"003a";
        if self.is_separator(last) || bare_drive {
            Some(tail)
        } else if self.is_separator(tail.as_bytes().get(..width)?) {
            tail.get(width..)
        } else {
            None
        }
    }

    pub(crate) fn is_direct_child(&self, parent: &str, path: &str) -> bool {
        self.relative(parent, path).is_some_and(|tail| {
            let width = self.width();
            let (dot, dots) = if self.windows {
                ("002e", "002e002e")
            } else {
                ("2e", "2e2e")
            };
            !tail.is_empty()
                && tail.len().is_multiple_of(width)
                && tail != dot
                && tail != dots
                && tail
                    .as_bytes()
                    .chunks_exact(width)
                    .all(|unit| !self.is_separator(unit))
        })
    }

    /// Returns the skip observation if a rule matches this exact path.
    pub(crate) fn matching_rule(&self, root: &str, path: &str) -> Option<bool> {
        let tail = self.relative(root, path)?;
        let index = self
            .keys
            .binary_search_by(|(key, _)| {
                if self.windows {
                    let units = tail
                        .as_bytes()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|unit| if unit == b"002f" { b"005c" } else { unit });
                    key.as_bytes().as_chunks::<4>().0.iter().cmp(units)
                } else {
                    key.as_str().cmp(tail)
                }
            })
            .ok()?;
        Some(self.keys[index].1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_paths_and_normalizes_for_the_source_platform() {
        for input in [
            "",
            ".",
            "./",
            "a/..",
            "../cache",
            "a/../../cache",
            "/cache",
            "a\0b",
        ] {
            assert_eq!(normalize(input, false), None, "{input:?}");
            assert_eq!(normalize(input, true), None, "{input:?}");
        }
        for input in [
            r"C:\cache",
            "C:cache",
            r"\cache",
            r"\\server\share",
            r"\\?\C:\cache",
            "a/COM¹.txt",
            "cache.",
            "foo/aux",
        ] {
            assert_eq!(normalize(input, true), None, "{input:?}");
        }
        assert_eq!(
            normalize("./a//./tmp/../cache/", false).as_deref(),
            Some("a/cache")
        );
        assert_eq!(normalize(r".\a//cache\", true).as_deref(), Some("a/cache"));
        assert_eq!(normalize(r"a\cache", false).as_deref(), Some(r"a\cache"));
    }

    #[test]
    fn keeps_only_topmost_exact_paths() {
        let mut rules = Exclusions::default();
        for path in [
            "cache-old",
            "cache/first",
            "cache/second",
            "./cache/",
            "cache",
            "cache/new",
            "other/cache",
            ".git",
        ] {
            rules.add(path).unwrap();
        }
        assert_eq!(rules.paths(), [".git", "cache", "cache-old", "other/cache"]);
        rules.remove(1);
        assert_eq!(rules.paths(), [".git", "cache-old", "other/cache"]);
    }

    #[test]
    fn windows_native_rules_match_components_without_changing_name_spelling() {
        let rules = [FolderExclusion {
            path: "packages/caché".into(),
            skipped: true,
        }];
        let paths = NativeExclusions::new(true, rules.iter());
        let locator = |name: &str| {
            let mut result = String::from("windows-utf16:");
            for unit in name.encode_utf16() {
                write!(result, "{unit:04x}").unwrap();
            }
            result
        };
        let root = locator("C:/");
        let parent = locator("C:/packages");
        let matching = locator(r"C:/packages\caché");
        assert!(paths.is_direct_child(&parent, &matching));
        assert_eq!(paths.matching_rule(&root, &matching), Some(true));
        assert_eq!(
            paths.matching_rule(&root, &locator("C:/packages/caché")),
            Some(true)
        );
        assert_eq!(
            paths.matching_rule(&root, &locator(r"C:/packages\Caché")),
            None
        );
        assert_eq!(
            paths.matching_rule(&root, &locator(r"C:/packages\caché-old")),
            None
        );
        assert!(!paths.is_direct_child(&root, &matching));
        assert!(paths.is_direct_child(&locator("C:"), &locator("C:packages")));
        assert_eq!(
            paths.matching_rule(&locator("C:"), &locator(r"C:packages\caché")),
            Some(true)
        );
        assert!(!paths.is_direct_child(&locator("C:"), &locator(r"C:\packages")));
    }
}
