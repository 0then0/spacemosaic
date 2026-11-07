//! Storage exploration UI. The list and treemap share a single selection and directory.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::preferences::WindowState;
use crate::{
    bridge::{self, Listing},
    i18n,
    preferences::{self, Preferences},
    scan::{self, Kind, ScanResult, Sort},
    theme::{self, Appearance},
};
mod app;
mod navigation;
mod phone;
mod saved;
mod search;
mod tree;
mod treemap;
#[cfg(test)]
use crate::compare::Dimension;
use eframe::egui::{self, RichText};
use phone::{PhoneOverlay, PhoneView};
use saved::{Changes, OpenTarget, SavedResult, View};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::path::Path;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Directory,
    Largest,
    Extensions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExtensionColumn {
    Name,
    Size,
    Files,
}
impl ExtensionColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Name => "extension",
            Self::Size => "size",
            Self::Files => "file_count",
        }
    }
}

pub struct ExplorerApp {
    compact_layout: bool,
    phone_home: bool,
    phone_view: PhoneView,
    phone_overlay: Option<PhoneOverlay>,
    language: usize,
    preferences: Preferences,
    preferences_path: Option<PathBuf>,
    saved_preferences: Preferences,
    last_preferences_write: Instant,
    preferences_write_failed: bool,
    settings_open: bool,
    tree: tree::TreeState,
    appearance: Appearance,
    service: scan::Service,
    job: Option<scan::Job>,
    /// The result on screen: the latest scan or the opened saved result.
    result: Option<Arc<ScanResult>>,
    /// Result of the latest scan started in this session.
    live: Option<Arc<ScanResult>>,
    view: View,
    saved: Option<SavedResult>,
    /// A second opened file, so two snapshots can be compared without a scan.
    second: Option<SavedResult>,
    open_job: Option<crate::job::OpenJob>,
    open_name: String,
    open_target: OpenTarget,
    /// Further files to read after the current one, when several were given at once.
    open_queue: std::collections::VecDeque<PathBuf>,
    open_error: Option<crate::import::ImportError>,
    changes: Changes,
    /// Id of the resizable map panel, which depends on its parent ui in egui 0.37.
    #[cfg(test)]
    map_panel_id: Option<egui::Id>,
    #[cfg(any(target_os = "android", target_os = "ios"))]
    native_open: Option<mpsc::Receiver<crate::mobile::OpenOutcome>>,
    #[cfg(any(target_os = "android", target_os = "ios"))]
    native_open_target: OpenTarget,
    /// Test hook: total bytes saved-result work may use where no memory reading exists.
    #[cfg(test)]
    memory_budget: Option<usize>,
    scope: Option<PathBuf>,
    provider_scope: Option<(u64, String)>,
    navigation: navigation::NavigationState,
    search: search::SearchState,
    sort: Sort,
    allocated: bool,
    cross_filesystems: bool,
    rows: Vec<usize>,
    extensions: Vec<(String, u64, scan::Measure)>,
    extension_sort: (ExtensionColumn, bool), // Column and descending order.
    dirty: bool,
    preview: Option<Listing>,
    mobile_busy: bool,
    notice: Option<&'static str>,
    export: Option<mpsc::Receiver<crate::mobile::ExportOutcome>>,
    map: treemap::MapState,
    #[cfg(any(target_os = "android", target_os = "ios"))]
    native_settings: Option<(Appearance, usize)>,
}
// Presentation only: native actions and exports retain the original locator.
fn readable_locator(locator: &str) -> String {
    let mut decoded = Vec::with_capacity(locator.len());
    let mut bytes = locator.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let (Some(a), Some(b)) = (bytes.next(), bytes.next()) else {
                return locator.into();
            };
            let (Some(a), Some(b)) = ((a as char).to_digit(16), (b as char).to_digit(16)) else {
                return locator.into();
            };
            decoded.push((a * 16 + b) as u8);
        } else {
            decoded.push(byte);
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| locator.into())
}
// Stable colors keep the same folder or file type recognizable across navigation.
fn tile_color(node: Option<&scan::Node>) -> egui::Color32 {
    let palette = [
        0x285b87, 0x7850a1, 0x267065, 0x95602b, 0x93465e, 0x495fa1, 0x556c2c, 0x8b4d31,
    ];
    let Some(node) = node else {
        return egui::Color32::from_gray(72);
    };
    let key = if node.kind == Kind::Directory {
        node.locator.clone()
    } else {
        PathBuf::from(&node.name)
            .extension()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    let hash = key.bytes().fold(2166136261_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16777619)
    });
    let color = palette[hash as usize % palette.len()];
    egui::Color32::from_rgb((color >> 16) as u8, (color >> 8) as u8, color as u8)
}
fn kind_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Directory => "directory",
        Kind::File => "file",
        Kind::Link => "link",
        Kind::Other => "other",
    }
}
fn measure(language: usize, size: scan::Measure) -> String {
    i18n::measure(language, size)
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn reveal(path: &Path) -> bool {
    let parent = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    #[cfg(target_os = "macos")]
    let command = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(target_os = "windows")]
    let command = std::process::Command::new("explorer.exe")
        .arg(parent)
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let command = std::process::Command::new("xdg-open").arg(parent).spawn();
    let _ = parent;
    command.is_ok()
}

#[cfg(test)]
mod tests;
