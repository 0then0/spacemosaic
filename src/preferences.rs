//! Local UI preferences. Scan metadata and storage locators are never persisted here.
use crate::{i18n, theme::Appearance};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Preferences {
    pub schema_version: u32,
    /// None follows the system language, including after the system language changes.
    pub language: Option<String>,
    pub appearance: Appearance,
    pub window: Option<WindowState>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            schema_version: 1,
            language: None,
            appearance: Appearance::System,
            window: None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct WindowState {
    /// Physical pixels, including the native frame. None on Wayland.
    pub position: Option<[i32; 2]>,
    /// Native logical pixels, independent of the application's text zoom.
    pub size: [f32; 2],
    pub maximized: bool,
}
impl WindowState {
    pub fn valid(&self) -> bool {
        self.size
            .into_iter()
            .all(|v| v.is_finite() && (64.0..=32768.0).contains(&v))
    }
    /// Preserve the restored (non-maximized) bounds when minimizing or maximizing.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn capture(
        previous: Option<Self>,
        viewport: &eframe::egui::ViewportInfo,
        size: eframe::egui::Vec2,
        zoom: f32,
    ) -> Option<Self> {
        if viewport.minimized == Some(true) || viewport.fullscreen == Some(true) {
            return previous;
        }
        if viewport.maximized == Some(true) {
            return previous.map(|mut state| {
                state.maximized = true;
                state
            });
        }
        let scale = viewport.native_pixels_per_point.unwrap_or(1.0) * zoom;
        let state = Self {
            position: viewport
                .outer_rect
                .map(|rect| {
                    [
                        (rect.min.x * scale).round() as i32,
                        (rect.min.y * scale).round() as i32,
                    ]
                })
                .or(previous.and_then(|s| s.position)),
            size: [size.x * zoom, size.y * zoom],
            maximized: false,
        };
        state.valid().then_some(state).or(previous)
    }
    /// Keep the title bar and restored bounds on an available monitor.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn fit(&self, origin: [i32; 2], extent: [u32; 2], scale: f64) -> ([i32; 2], [u32; 2]) {
        let size = std::array::from_fn(|i| {
            ((self.size[i] as f64 * scale).round() as u32)
                .max(1)
                .min(extent[i].max(1))
        });
        let position = std::array::from_fn(|i| {
            self.position.unwrap_or(origin)[i].clamp(
                origin[i],
                origin[i].saturating_add(extent[i].saturating_sub(size[i]) as i32),
            )
        });
        (position, size)
    }
}
impl Preferences {
    pub fn load(path: &Path) -> Self {
        let read = || -> Option<Self> {
            let mut source = String::new();
            std::fs::File::open(path)
                .ok()?
                .take(65537)
                .read_to_string(&mut source)
                .ok()?;
            if source.len() > 65536 {
                return None;
            }
            let mut value: Self = serde_json::from_str(&source).ok()?;
            if value.schema_version != 1 {
                return None;
            }
            if value
                .language
                .as_deref()
                .is_some_and(|tag| !i18n::TAGS.contains(&tag))
            {
                value.language = None;
            }
            if value.window.is_some_and(|window| !window.valid()) {
                value.window = None;
            }
            Some(value)
        };
        read().unwrap_or_default()
    }
    pub fn language_index(&self) -> usize {
        self.language
            .as_deref()
            .and_then(|tag| i18n::TAGS.iter().position(|candidate| *candidate == tag))
            .unwrap_or_else(i18n::system_language)
    }
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("preferences need a parent directory"))?;
        std::fs::create_dir_all(parent)?;
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let write = || -> std::io::Result<()> {
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, path)
        };
        let outcome = write();
        if outcome.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        outcome
    }
}

pub(crate) fn path() -> Option<PathBuf> {
    #[cfg(target_os = "android")]
    let base = crate::android::preferences_directory();
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("SpaceMosaic"));
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| p.join("Library/Application Support/SpaceMosaic"));
    #[cfg(not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    )))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .map(|p| p.join("spacemosaic"));
    base.filter(|p| p.is_absolute())
        .map(|p| p.join("settings.json"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    pub(crate) struct Fixture(PathBuf);
    impl Fixture {
        pub fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "spacemosaic-preferences-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&directory).unwrap();
            Self(directory)
        }
        pub fn path(&self) -> PathBuf {
            self.0.join("settings.json")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn window() -> WindowState {
        WindowState {
            position: Some([-1400, 120]),
            size: [1000.0, 700.0],
            maximized: false,
        }
    }
    #[test]
    fn repeated_saves_restore_all_languages_themes_and_window_bounds() {
        let files = Fixture::new();
        for tag in i18n::TAGS {
            for appearance in Appearance::ALL {
                let settings = Preferences {
                    language: Some(tag.into()),
                    appearance,
                    window: Some(window()),
                    ..Default::default()
                };
                settings.save(&files.path()).unwrap();
                assert_eq!(Preferences::load(&files.path()), settings);
                assert_eq!(
                    i18n::TAGS[Preferences::load(&files.path()).language_index()],
                    tag
                );
            }
        }
        Preferences::default().save(&files.path()).unwrap();
        assert_eq!(Preferences::load(&files.path()), Preferences::default());
        assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), 1);
    }
    #[test]
    fn missing_corrupt_future_and_oversized_settings_use_defaults() {
        let files = Fixture::new();
        assert_eq!(Preferences::load(&files.path()), Preferences::default());
        for source in [
            "{truncated".into(),
            r#"{"schema_version":2,"language":"ru"}"#.into(),
            " ".repeat(65537),
        ] {
            std::fs::write(files.path(), source).unwrap();
            assert_eq!(Preferences::load(&files.path()), Preferences::default());
        }
        std::fs::write(files.path(), r#"{"language":"unsupported","appearance":"dark","window":{"position":[1,2],"size":[0,900],"maximized":false}}"#).unwrap();
        let loaded = Preferences::load(&files.path());
        assert_eq!(loaded.language, None);
        assert_eq!(loaded.window, None);
        assert_eq!(loaded.appearance, Appearance::Dark);
    }
    #[test]
    fn write_failure_is_reported_and_does_not_overwrite_existing_settings() {
        let files = Fixture::new();
        Preferences::default().save(&files.path()).unwrap();
        assert!(
            Preferences::default()
                .save(&files.path().join("blocked.json"))
                .is_err()
        );
        assert_eq!(Preferences::load(&files.path()), Preferences::default());
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn window_capture_preserves_normal_bounds_and_handles_zoom_and_wayland() {
        use eframe::egui::{Rect, ViewportInfo, pos2, vec2};
        let mut viewport = ViewportInfo {
            maximized: Some(true),
            ..Default::default()
        };
        let mut expected = window();
        expected.maximized = true;
        assert_eq!(
            WindowState::capture(Some(window()), &viewport, vec2(1920.0, 1080.0), 1.0),
            Some(expected)
        );
        viewport.minimized = Some(true);
        assert_eq!(
            WindowState::capture(Some(expected), &viewport, vec2(0.0, 0.0), 1.0),
            Some(expected)
        );
        viewport.minimized = Some(false);
        viewport.maximized = Some(false);
        viewport.native_pixels_per_point = Some(2.0);
        viewport.outer_rect = Some(Rect::from_min_size(pos2(-400.0, 40.0), vec2(600.0, 450.0)));
        let state =
            WindowState::capture(Some(expected), &viewport, vec2(600.0, 450.0), 1.5).unwrap();
        assert_eq!(state.position, Some([-1200, 120]));
        assert_eq!(state.size, [900.0, 675.0]);
        assert!(!state.maximized);
        viewport.outer_rect = None;
        assert_eq!(
            WindowState::capture(None, &viewport, vec2(600.0, 450.0), 1.0)
                .unwrap()
                .position,
            None
        );
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn disconnected_monitor_and_changed_dpi_keep_window_visible() {
        assert_eq!(
            window().fit([0, 0], [1920, 1080], 1.0),
            ([0, 120], [1000, 700])
        );
        assert_eq!(
            window().fit([-1920, 0], [1920, 1080], 1.0),
            ([-1400, 120], [1000, 700])
        );
        assert_eq!(
            window().fit([0, 0], [1280, 720], 2.0),
            ([0, 0], [1280, 720])
        );
    }
}
