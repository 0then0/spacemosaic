//! Shared mobile result access, native commands, and system-save handoff.
use crate::{bridge, provider, scan};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

/// Physical window geometry reported by Android. The render surface can either
/// fill the window or already exclude native decor/IME; do not subtract it twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowInsets {
    pub system: [i32; 4],
    pub content: [i32; 4],
    pub window: [i32; 2],
    pub footer_top: i32,
    pub system_dark: bool,
}
impl WindowInsets {
    pub fn margins(self, surface: [i32; 2]) -> [f32; 4] {
        let origin = if (surface[0] - self.content[2]).abs() <= 1
            && (surface[1] - self.content[3]).abs() <= 1
        {
            [self.content[0], self.content[1]]
        } else {
            [0, 0]
        };
        [
            (self.system[0] - origin[0]).max(0) as f32,
            (self.system[1] - origin[1]).max(0) as f32,
            (origin[0] + surface[0] - (self.window[0] - self.system[2])).max(0) as f32,
            (origin[1] + surface[1] - (self.window[1] - self.system[3]).min(self.footer_top)).max(0)
                as f32,
        ]
    }
}

#[derive(Clone, Copy)]
pub enum Command {
    Rescan,
    Cancel,
    Export(bool),
    Navigate(usize),
    Select(usize),
    Back,
    CloseSettings,
    Language(usize),
    Appearance(usize),
    Allocated(bool),
}
fn commands() -> &'static Mutex<VecDeque<(u64, Command)>> {
    static VALUE: OnceLock<Mutex<VecDeque<(u64, Command)>>> = OnceLock::new();
    VALUE.get_or_init(Mutex::default)
}
pub fn take_command() -> Option<Command> {
    let mut queue = commands().lock().unwrap_or_else(|p| p.into_inner());
    while let Some((generation, command)) = queue.pop_front() {
        if bridge::is_current(generation) {
            return Some(command);
        }
    }
    None
}

#[derive(Default)]
struct State {
    generation: u64,
    result: Option<Arc<scan::ScanResult>>,
    progress: Option<Arc<Mutex<scan::Progress>>>,
    directory: usize,
    can_back: bool,
    settings_open: bool,
}
fn state() -> &'static Mutex<State> {
    static VALUE: OnceLock<Mutex<State>> = OnceLock::new();
    VALUE.get_or_init(Mutex::default)
}
pub fn clear(generation: u64) {
    *state().lock().unwrap_or_else(|p| p.into_inner()) = State {
        generation,
        ..Default::default()
    };
}
pub fn scanning(generation: u64, progress: Arc<Mutex<scan::Progress>>) {
    *state().lock().unwrap_or_else(|p| p.into_inner()) = State {
        generation,
        progress: Some(progress),
        ..Default::default()
    };
}
pub fn publish(generation: u64, result: Arc<scan::ScanResult>) {
    if bridge::is_current(generation) {
        *state().lock().unwrap_or_else(|p| p.into_inner()) = State {
            generation,
            result: Some(result),
            ..Default::default()
        };
    }
}
pub fn focus(directory: usize, grouped: bool, settings_open: bool) {
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    state.directory = directory;
    state.can_back = directory != 0 || grouped;
    state.settings_open = settings_open;
}
pub fn back() -> bool {
    let (generation, can_back, settings_open) = {
        let state = state().lock().unwrap_or_else(|p| p.into_inner());
        (state.generation, state.can_back, state.settings_open)
    };
    (can_back || settings_open)
        && bridge::is_current(generation)
        && enqueue(
            generation,
            if settings_open {
                Command::CloseSettings
            } else {
                Command::Back
            },
        )
}
fn enqueue(generation: u64, command: Command) -> bool {
    let mut queue = commands().lock().unwrap_or_else(|p| p.into_inner());
    if queue.len() >= 8 {
        return false;
    }
    queue.push_back((generation, command));
    if matches!(command, Command::Rescan) {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        state.result = None;
        state.progress = Some(Arc::new(Mutex::new(scan::Progress::default())));
    }
    drop(queue);
    bridge::wake();
    true
}

/// Read only completed in-memory results. Native lists never walk storage.
pub fn page(request: &str) -> String {
    if request.len() > 8192 {
        return "{}".into();
    }
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    if let Some(command) = request["command"].as_str() {
        let generation = request["generation"]
            .as_u64()
            .unwrap_or_else(bridge::generation);
        if !bridge::is_current(generation) {
            return "{}".into();
        }
        let index = request["index"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(0);
        let command = match command {
            "rescan" => Command::Rescan,
            "cancel" => Command::Cancel,
            "json" => Command::Export(true),
            "csv" => Command::Export(false),
            "navigate" => Command::Navigate(index),
            "select" => Command::Select(index),
            "language" => Command::Language(index),
            "appearance" => Command::Appearance(index),
            "allocated" => Command::Allocated(index != 0),
            _ => return "{}".into(),
        };
        if matches!(command, Command::Navigate(_) | Command::Select(_))
            && state()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .result
                .is_none()
        {
            return json!({"accepted":false}).to_string();
        }
        return json!({"accepted":enqueue(generation,command)}).to_string();
    }
    let (generation, result, progress) = {
        let state = state().lock().unwrap_or_else(|p| p.into_inner());
        (
            state.generation,
            state.result.clone(),
            state.progress.clone(),
        )
    };
    if !bridge::is_current(generation) {
        return json!({"status":"waiting", "entries":[]}).to_string();
    }
    let Some(result) = result else {
        let progress = progress.map(|p| p.lock().unwrap_or_else(|e| e.into_inner()).clone());
        let language = request["language"]
            .as_str()
            .and_then(|tag| crate::i18n::TAGS.iter().position(|t| *t == tag))
            .unwrap_or(0);
        return json!({"generation":generation, "status":if progress.is_some() {"scanning"} else {"waiting"},
            "summary":summary(language,progress.as_ref().map(|p|p.objects).unwrap_or(0),progress.as_ref().map(|p|p.known_bytes).unwrap_or(0),progress.as_ref().map(|p|p.errors).unwrap_or(0)),
            "objects":progress.as_ref().map(|p| p.objects).unwrap_or(0),
            "known_bytes":progress.as_ref().map(|p| p.known_bytes).unwrap_or(0),
            "errors":progress.as_ref().map(|p| p.errors).unwrap_or(0), "entries":[]}).to_string();
    };
    result_page(&result, generation, &request).to_string()
}
fn result_page(result: &scan::ScanResult, generation: u64, request: &Value) -> Value {
    let directory = request["directory"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .filter(|i| {
            result
                .nodes
                .get(*i)
                .is_some_and(|n| n.kind == scan::Kind::Directory)
        })
        .unwrap_or(0);
    let query = request["query"].as_str().unwrap_or("");
    let mode = request["mode"].as_str().unwrap_or("directory");
    let language = request["language"]
        .as_str()
        .and_then(|tag| crate::i18n::TAGS.iter().position(|t| *t == tag))
        .unwrap_or(0);
    let sort = if request["sort"].as_str() == Some("name") {
        scan::Sort::Name
    } else {
        scan::Sort::Size
    };
    let offset = request["offset"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(0);
    let (total, offset, mut entries) = if mode == "extensions" {
        let extensions: Vec<_> = result
            .extensions()
            .into_iter()
            .filter(|(name, _, _)| name.to_lowercase().contains(&query.to_lowercase()))
            .collect();
        let offset = offset.min(extensions.len());
        let entries: Vec<_> = extensions.iter().skip(offset).take(provider::PAGE_ENTRIES)
            .map(|(name, count, size)| json!({"name":if name.is_empty() {crate::i18n::text(language,"no_extension")} else {name},
                "count_label":crate::i18n::counted(language,*count,"file"),"size_label":display_measure(language,*size),
                "directory":false})).collect();
        (extensions.len(), offset, entries)
    } else {
        let mut rows = if !query.is_empty() || mode == "largest" {
            result.search(query, mode == "largest", sort)
        } else {
            result.children(directory, "", sort)
        };
        if request["allocated"].as_bool() == Some(true) && sort == scan::Sort::Size {
            rows.sort_by_key(|index| std::cmp::Reverse(result.nodes[*index].allocated.known));
        }
        let offset = offset.min(rows.len());
        let entries: Vec<_> = rows.iter().skip(offset).take(provider::PAGE_ENTRIES).map(|index| {
            let n = &result.nodes[*index];
            json!({"id":index,"name":n.name,"locator":n.locator,"directory":n.kind == scan::Kind::Directory,
                "size_label":display_measure(language,if request["allocated"].as_bool()==Some(true) {n.allocated} else {n.logical}),"logical_label":display_measure(language,n.logical),"allocated_label":display_measure(language,n.allocated),"hard_link":n.hard_link,
                "complete":n.complete,"error":n.error})
        }).collect();
        (rows.len(), offset, entries)
    };
    let mut response = json!({"generation":generation,"status":if result.cancelled || result.limited || !result.nodes[0].complete {"partial"} else {"complete"},
        "scope":result.scope,"summary":format!("{} · {} · {}",crate::i18n::counted(language,result.nodes.len() as u64,"object"),display_measure(language,result.nodes[0].logical),crate::i18n::counted(language,result.errors,"error")),
        "directory":directory,"parent":result.nodes[directory].parent,"name":result.nodes[directory].name,
        "offset":offset,"total":total});
    loop {
        response["next"] = json!(offset + entries.len());
        response["has_more"] = json!(offset + entries.len() < total);
        response["entries"] = json!(entries);
        if response.to_string().len() <= provider::PAGE_BYTES {
            return response;
        }
        if entries.len() <= 1 {
            return json!({"status":"unavailable","error":"message_too_large","entries":[]});
        }
        entries.truncate(entries.len() / 2);
    }
}
fn display_measure(language: usize, measure: scan::Measure) -> String {
    if measure.overflow {
        return crate::i18n::text(language, "overflow").into();
    }
    if measure.known == 0 && measure.unknown > 0 {
        return crate::i18n::text(language, "unknown").into();
    }
    let mut text = crate::i18n::bytes(language, measure.known);
    if measure.unknown > 0 {
        text.push_str(&format!(
            " + {} {}",
            crate::i18n::integer(language, measure.unknown),
            crate::i18n::text(language, "unknown")
        ));
    }
    text
}
fn summary(language: usize, objects: usize, known: u64, errors: u64) -> String {
    format!(
        "{} · {} · {}",
        crate::i18n::counted(language, objects as u64, "object"),
        crate::i18n::bytes(language, known),
        crate::i18n::counted(language, errors, "error")
    )
}

pub fn transport() -> provider::Transport {
    #[cfg(all(feature = "gui", target_os = "android"))]
    {
        crate::android::storage_transport()
    }
    #[cfg(all(feature = "gui", target_os = "ios"))]
    {
        Arc::new(crate::ios::request)
    }
    #[cfg(not(all(feature = "gui", any(target_os = "android", target_os = "ios"))))]
    {
        Arc::new(|_| Err("provider_unavailable".into()))
    }
}
pub fn request(value: Value) -> Result<Value, String> {
    let response = transport()(&value.to_string())?;
    if response.len() > provider::PAGE_BYTES {
        return Err("message_too_large".into());
    }
    serde_json::from_str(&response).map_err(|_| "metadata_unavailable".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportOutcome {
    Saved,
    Cancelled,
    Failed,
}
impl ExportOutcome {
    pub fn code(code: i32) -> Self {
        match code {
            1 => Self::Saved,
            2 => Self::Cancelled,
            _ => Self::Failed,
        }
    }
}
struct Export {
    id: u64,
    sender: mpsc::SyncSender<ExportOutcome>,
    path: Option<PathBuf>,
}
fn exports() -> &'static Mutex<Option<Export>> {
    static VALUE: OnceLock<Mutex<Option<Export>>> = OnceLock::new();
    VALUE.get_or_init(Mutex::default)
}
pub fn export(
    result: Arc<scan::ScanResult>,
    json_format: bool,
) -> Option<mpsc::Receiver<ExportOutcome>> {
    static SERIAL: AtomicU64 = AtomicU64::new(1);
    let id = SERIAL
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
        .ok()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let mut pending = exports().lock().unwrap_or_else(|p| p.into_inner());
    if pending.is_some() {
        return None;
    }
    *pending = Some(Export {
        id,
        sender,
        path: None,
    });
    drop(pending);
    std::thread::spawn(move || {
        let outcome = (|| -> Result<(), String> {
            let response = request(json!({"op":"cache", "id":id,"json":json_format}))?;
            let path = PathBuf::from(response["path"].as_str().ok_or("export_failed")?);
            scan::write_export(&result, &path, json_format).map_err(|_| "export_failed")?;
            if let Some(export) = exports()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_mut()
                .filter(|e| e.id == id)
            {
                export.path = Some(path.clone());
            } else {
                let _ = std::fs::remove_file(&path);
                return Err("export_failed".into());
            }
            let response = request(json!({"op":"save", "id":id,"path":path,"json":json_format}))?;
            if response["accepted"].as_bool() != Some(true) {
                return Err("export_failed".into());
            }
            Ok(())
        })();
        if outcome.is_err() {
            export_complete(id, ExportOutcome::Failed);
        }
    });
    Some(receiver)
}
pub fn export_complete(id: u64, outcome: ExportOutcome) {
    let mut pending = exports().lock().unwrap_or_else(|p| p.into_inner());
    if pending.as_ref().is_some_and(|e| e.id == id) {
        let export = pending.take().expect("matching export");
        drop(pending);
        if let Some(path) = export.path {
            let _ = std::fs::remove_file(path);
        }
        let _ = export.sender.try_send(outcome);
        bridge::wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Kind, Measure, Node, ScanResult};
    fn result(count: usize) -> ScanResult {
        let nodes = (0..=count)
            .map(|i| Node {
                parent: if i == 0 { None } else { Some(0) },
                name: format!("file-{i}.dat"),
                locator: format!("content://fixture/document/{i}"),
                native_locator: format!("content://fixture/document/{i}"),
                kind: if i == 0 { Kind::Directory } else { Kind::File },
                logical: Measure {
                    known: i as u64,
                    ..Default::default()
                },
                allocated: Measure {
                    unknown: 1,
                    ..Default::default()
                },
                complete: true,
                hard_link: false,
                error: None,
                path: None,
            })
            .collect();
        ScanResult {
            schema_version: 1,
            started_unix_seconds: 0,
            elapsed_millis: 1,
            scope: "content://fixture/root".into(),
            cancelled: false,
            limited: false,
            cross_filesystems: false,
            errors: 0,
            nodes,
        }
    }
    #[test]
    fn android_insets_respect_edge_to_edge_fitted_surfaces_ime_and_rotation() {
        let full = WindowInsets {
            system: [0, 90, 0, 60],
            content: [0, 0, 1080, 2400],
            window: [1080, 2400],
            footer_top: 2172,
            system_dark: false,
        };
        assert_eq!(full.margins([1080, 2400]), [0.0, 90.0, 0.0, 228.0]);
        let fitted = WindowInsets {
            content: [0, 90, 1080, 2250],
            ..full
        };
        assert_eq!(fitted.margins([1080, 2250]), [0.0, 0.0, 0.0, 168.0]);
        let keyboard = WindowInsets {
            system: [0, 90, 0, 900],
            content: [0, 0, 1080, 1500],
            footer_top: 1332,
            ..full
        };
        assert_eq!(keyboard.margins([1080, 1500]), [0.0, 90.0, 0.0, 168.0]);
        assert_eq!(keyboard.margins([1080, 2400]), [0.0, 90.0, 0.0, 1068.0]);
        let rotated = WindowInsets {
            system: [90, 0, 60, 0],
            content: [0, 0, 2400, 1080],
            window: [2400, 1080],
            footer_top: 912,
            system_dark: false,
        };
        assert_eq!(rotated.margins([2400, 1080]), [90.0, 0.0, 60.0, 168.0]);
    }
    #[test]
    fn native_results_are_paged_sorted_and_filtered_without_storage_access() {
        let result = result(1000);
        let first = result_page(&result, 7, &json!({"language":"ru"}));
        assert_eq!(first["entries"].as_array().unwrap().len(), 32);
        assert_eq!(first["entries"][0]["id"], 1000);
        assert_eq!(first["next"], 32);
        assert_eq!(first["total"], 1000);
        assert_eq!(first["has_more"], true);
        let next = result_page(&result, 7, &json!({"offset":32}));
        assert_ne!(next["entries"][0]["id"], first["entries"][0]["id"]);
        let filtered = result_page(
            &result,
            7,
            &json!({"mode":"largest","query":"file-123.dat"}),
        );
        assert_eq!(filtered["total"], 1);
        assert_eq!(
            filtered["entries"][0]["locator"],
            "content://fixture/document/123"
        );
        assert!(
            filtered["entries"][0]["allocated_label"]
                .as_str()
                .unwrap()
                .contains("Unknown")
        );
        let types = result_page(&result, 7, &json!({"mode":"extensions"}));
        assert_eq!(types["entries"][0]["name"], "dat");
        assert!(types["entries"][0]["id"].is_null());
    }
    #[test]
    fn native_page_bytes_are_bounded_and_next_offset_matches_the_returned_page() {
        let mut result = result(100);
        for node in &mut result.nodes[1..] {
            node.locator.push_str(&"x".repeat(40 * 1024));
        }
        let page = result_page(&result, 8, &json!({}));
        let count = page["entries"].as_array().unwrap().len();
        assert!(count > 0 && count < provider::PAGE_ENTRIES);
        assert_eq!(page["next"], count);
        assert!(page.to_string().len() <= provider::PAGE_BYTES);
        let next = result_page(&result, 8, &json!({"offset":count}));
        assert_ne!(page["entries"][0]["id"], next["entries"][0]["id"]);
    }
    #[test]
    fn native_commands_reject_old_generations_and_old_result_rows_after_rescan() {
        let generation = bridge::begin();
        publish(generation, Arc::new(result(2)));
        let accepted: Value = serde_json::from_str(&page(
            &json!({"command":"navigate","index":1,"generation":generation}).to_string(),
        ))
        .unwrap();
        assert_eq!(accepted["accepted"], true);
        let newer = bridge::begin();
        assert!(take_command().is_none());
        assert_eq!(
            page(&json!({"command":"cancel","generation":generation}).to_string()),
            "{}"
        );
        publish(newer, Arc::new(result(2)));
        page(&json!({"command":"rescan","generation":newer}).to_string());
        let rejected: Value = serde_json::from_str(&page(
            &json!({"command":"select","index":1,"generation":newer}).to_string(),
        ))
        .unwrap();
        assert_eq!(rejected["accepted"], false);
        assert!(matches!(take_command(), Some(Command::Rescan)));
        assert!(take_command().is_none());
        focus(0, false, true);
        assert!(
            back(),
            "Android Back must dismiss settings before leaving the app"
        );
        assert!(matches!(take_command(), Some(Command::CloseSettings)));
        focus(0, false, false);
        assert!(!back());
        assert_eq!(
            serde_json::from_str::<Value>(&page("{}")).unwrap()["status"],
            "scanning"
        );
    }
    #[test]
    fn save_cancellation_is_distinct_and_stale_callbacks_cannot_delete_current_exports() {
        let path = std::env::temp_dir().join(format!(
            "spacemosaic-export-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"{}").unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        *exports().lock().unwrap() = Some(Export {
            id: 9001,
            sender,
            path: Some(path.clone()),
        });
        export_complete(9000, ExportOutcome::Failed);
        assert!(path.exists());
        assert!(receiver.try_recv().is_err());
        export_complete(9001, ExportOutcome::Cancelled);
        assert!(!path.exists());
        assert_eq!(receiver.try_recv().unwrap(), ExportOutcome::Cancelled);
        assert!(exports().lock().unwrap().is_none());
    }
}
