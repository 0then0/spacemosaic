//! Shared mobile result access, native commands, and system-save handoff.
use crate::i18n::measure as display_measure;
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

#[derive(Clone)]
pub enum Command {
    Search(String),
    Rescan,
    ScanFolder(usize),
    Memory(crate::memory::BudgetMode),
    Cancel,
    Export(bool),
    Navigate(usize),
    Select(usize),
    Back,
    CloseSettings,
    Language(usize),
    Appearance(usize),
    Allocated(bool),
    OpenSaved,
    /// Choose a file for one side of a comparison.
    OpenFor(crate::compare::Side),
    SwapDirection,
    ConfirmScope(bool),
    StartCompare,
    /// Browse the result that stands on one side of the comparison.
    ShowSide(crate::compare::Side),
}
fn memory_setting() -> &'static Mutex<crate::memory::BudgetMode> {
    static SETTING: OnceLock<Mutex<crate::memory::BudgetMode>> = OnceLock::new();
    SETTING.get_or_init(Mutex::default)
}
pub fn configure_memory(mode: crate::memory::BudgetMode) {
    *memory_setting().lock().unwrap_or_else(|p| p.into_inner()) = mode;
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
    /// The browsed result is an opened file: its recorded locations are never actionable.
    historical: bool,
    comparison: Option<Arc<crate::compare::Comparison>>,
    setup: Option<Setup>,
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
/// Show the result now on the canvas, whether live or opened from a file.
pub fn publish_view(
    generation: u64,
    result: Arc<scan::ScanResult>,
    historical: bool,
    comparison: Option<Arc<crate::compare::Comparison>>,
) {
    if bridge::is_current(generation) {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        state.generation = generation;
        state.result = Some(result);
        state.progress = None;
        state.historical = historical;
        state.comparison = comparison;
    }
}
/// Nothing is browsed any more (for example the saved result was closed).
pub fn clear_view(generation: u64) {
    if bridge::is_current(generation) {
        *state().lock().unwrap_or_else(|p| p.into_inner()) = State {
            generation,
            ..Default::default()
        };
    }
}
/// What the native comparison page shows about the choice of Before and After.
/// Plain facts only; the page localizes them for the request's language.
#[derive(Clone, Debug, Default)]
pub struct Setup {
    pub before: Option<SetupOperand>,
    pub after: Option<SetupOperand>,
    pub assessment: Option<crate::compare::ScopeAssessment>,
    pub confirmed: bool,
    pub comparing: bool,
    pub opening: bool,
    pub failure: Option<SetupFailure>,
    pub finished: bool,
}
#[derive(Clone, Debug)]
pub struct SetupOperand {
    pub name: String,
    pub live: bool,
    pub captured: u64,
    pub objects: u64,
    pub partial: bool,
}
#[derive(Clone, Debug)]
pub enum SetupFailure {
    Incompatible(crate::compare::ScopeAssessment),
    Other(&'static str),
}
/// Publish the current choice of comparison operands for the native browser.
pub fn publish_setup(generation: u64, setup: Setup) {
    if bridge::is_current(generation) {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        state.generation = generation;
        state.setup = Some(setup);
    }
}
fn setup_page(language: usize, setup: &Setup) -> Value {
    use crate::compare::ScopeVerdict;
    let t = |key| crate::i18n::text(language, key);
    let operand = |operand: &Option<SetupOperand>| match operand {
        Some(o) => format!(
            "{}: {} · {} · {} · {}",
            t(if o.live { "view_current" } else { "saved_file" }),
            o.name,
            crate::i18n::timestamp(o.captured),
            crate::i18n::counted(language, o.objects, "object"),
            t(if o.partial { "partial" } else { "complete" })
        ),
        None => t("compare_not_chosen").to_owned(),
    };
    let scope_text = |assessment: &crate::compare::ScopeAssessment| {
        assessment
            .notes
            .iter()
            .map(|note| t(note.key()))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut lines = vec![
        format!("{}: {}", t("compare_baseline"), operand(&setup.before)),
        format!("{}: {}", t("compare_current"), operand(&setup.after)),
    ];
    let ready = setup.before.is_some() && setup.after.is_some() && setup.assessment.is_some();
    let mut needs_confirmation = false;
    let mut can_start = false;
    if let Some(assessment) = setup.assessment.as_ref().filter(|_| ready) {
        lines.push(format!(
            "{} {} {}",
            t("compare_baseline"),
            t("compare_arrow"),
            t("compare_current")
        ));
        lines.push(
            t(match assessment.verdict {
                ScopeVerdict::Matching => "cmp_verdict_matching",
                ScopeVerdict::Insufficient => "cmp_verdict_insufficient",
                ScopeVerdict::Incompatible => "cmp_verdict_incompatible",
            })
            .to_owned(),
        );
        lines.push(scope_text(assessment));
        needs_confirmation = assessment.verdict == ScopeVerdict::Insufficient;
        can_start = !setup.comparing
            && !setup.opening
            && match assessment.verdict {
                ScopeVerdict::Matching => true,
                ScopeVerdict::Insufficient => setup.confirmed,
                ScopeVerdict::Incompatible => false,
            };
        for (operand, key) in [
            (&setup.before, "cmp_limit_baseline_partial"),
            (&setup.after, "cmp_limit_current_partial"),
        ] {
            if operand.as_ref().is_some_and(|o| o.partial) {
                lines.push(t(key).to_owned());
            }
        }
    } else {
        lines.push(t("compare_needs_two").to_owned());
    }
    if setup.comparing {
        lines.push(t("comparing").to_owned());
    }
    match &setup.failure {
        Some(SetupFailure::Incompatible(scope)) => {
            lines.push(t("cmp_verdict_incompatible").to_owned());
            lines.push(scope_text(scope));
        }
        Some(SetupFailure::Other(key)) => lines.push(t(key).to_owned()),
        None => {}
    }
    json!({
        "lines": lines,
        "can_swap": ready,
        "needs_confirmation": needs_confirmation,
        "confirmed": setup.confirmed,
        "can_start": can_start && !setup.finished,
        "can_show_before": setup.before.is_some(),
        "can_show_after": setup.after.is_some(),
        "busy": setup.opening,
    })
}
/// Advertise Back for the current canvas screen as well as folder navigation.
/// Native Android Back must dismiss a result/overlay before exiting the Activity.
pub fn focus(directory: usize, grouped: bool, settings_open: bool, can_view_back: bool) {
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    state.directory = directory;
    state.can_back = directory != 0 || grouped || can_view_back;
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
    let rescan = matches!(command, Command::Rescan | Command::ScanFolder(_));
    queue.push_back((generation, command));
    if rescan {
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
    if request["command"] == "memory_setting" {
        return serde_json::to_string(&*memory_setting().lock().unwrap_or_else(|p| p.into_inner()))
            .unwrap_or_default();
    }
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
            "search" => Command::Search(request["query"].as_str().unwrap_or("").to_owned()),
            "rescan" => Command::Rescan,
            "memory_automatic" => Command::Memory(crate::memory::BudgetMode::Automatic),
            "memory_manual" => {
                let Some(mib) = request["mib"].as_u64().and_then(|v| u32::try_from(v).ok()) else {
                    return json!({"accepted":false}).to_string();
                };
                let mode = crate::memory::BudgetMode::Manual { mib };
                if !mode.valid() {
                    return json!({"accepted":false}).to_string();
                }
                Command::Memory(mode)
            }
            "scan_folder" => {
                let state = state().lock().unwrap_or_else(|p| p.into_inner());
                if state.historical
                    || !state
                        .result
                        .as_ref()
                        .and_then(|r| r.nodes.get(index))
                        .is_some_and(|n| n.kind == scan::Kind::Directory)
                {
                    return json!({"accepted":false}).to_string();
                }
                Command::ScanFolder(index)
            }
            "cancel" => Command::Cancel,
            "json" => Command::Export(true),
            "csv" => Command::Export(false),
            "navigate" => Command::Navigate(index),
            "select" => Command::Select(index),
            "language" => Command::Language(index),
            "appearance" => Command::Appearance(index),
            "allocated" => Command::Allocated(index != 0),
            "open_saved" => Command::OpenSaved,
            "open_before" => Command::OpenFor(crate::compare::Side::Baseline),
            "open_after" => Command::OpenFor(crate::compare::Side::Current),
            "swap_direction" => Command::SwapDirection,
            "confirm_scope" => Command::ConfirmScope(index != 0),
            "start_compare" => Command::StartCompare,
            "show_before" => Command::ShowSide(crate::compare::Side::Baseline),
            "show_after" => Command::ShowSide(crate::compare::Side::Current),
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
    let (generation, result, progress, historical, comparison, setup) = {
        let state = state().lock().unwrap_or_else(|p| p.into_inner());
        (
            state.generation,
            state.result.clone(),
            state.progress.clone(),
            state.historical,
            state.comparison.clone(),
            state.setup.clone(),
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
    let mut page = result_page(&result, generation, &request, comparison.as_deref());
    page["historical"] = json!(historical);
    if request["mode"].as_str() == Some("changes") {
        let language = request["language"]
            .as_str()
            .and_then(|tag| crate::i18n::TAGS.iter().position(|t| *t == tag))
            .unwrap_or(0);
        let mut setup = setup.unwrap_or_default();
        setup.finished = comparison.is_some();
        page["compare"] = setup_page(language, &setup);
    }
    page.to_string()
}
fn changes_page(
    comparison: Option<&crate::compare::Comparison>,
    language: usize,
    request: &Value,
) -> (usize, usize, Vec<Value>, String) {
    use crate::compare::{ChangeSort, Dimension, Status};
    let Some(comparison) = comparison else {
        return (
            0,
            0,
            Vec::new(),
            crate::i18n::text(language, "cmp_none_native").into(),
        );
    };
    let query = request["query"].as_str().unwrap_or("");
    let sort = if request["sort"].as_str() == Some("name") {
        ChangeSort::Name
    } else {
        ChangeSort::Growth
    };
    let dimension =
        if request["allocated"].as_bool() == Some(true) && comparison.allocated_available {
            Dimension::Allocated
        } else {
            Dimension::Logical
        };
    let rows = comparison.events(dimension, Status::CHANGED, query, sort);
    let totals = comparison.totals(dimension);
    let summary = format!(
        "{} {} · {}",
        crate::i18n::text(
            language,
            if totals.is_complete() {
                "cmp_tile_net"
            } else {
                "cmp_tile_net_partial"
            }
        ),
        crate::i18n::signed_bytes(language, totals.net()),
        crate::i18n::counted(language, totals.unknown_count, "object")
    );
    let offset = request["offset"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(0)
        .min(rows.len());
    let entries = rows
        .iter()
        .skip(offset)
        .take(provider::PAGE_ENTRIES)
        .map(|&row| {
            let i = row as usize;
            let node = comparison.nodes[i];
            let status = comparison.event_status(i, dimension);
            let change = if status == Status::Unknown {
                crate::i18n::text(language, "unknown").to_owned()
            } else {
                crate::i18n::signed_bytes(language, node.confirmed(dimension))
            };
            let reason = node
                .reason(dimension)
                .key()
                .map(|key| format!("\n{}", crate::i18n::text(language, key)))
                .unwrap_or_default();
            json!({"name":comparison.name(i),
                "size_label":format!("{} {} · {}{}",status.symbol(),crate::i18n::text(language,status.key()),change,reason),
                "count_label":comparison.path(i),"directory":false})
        })
        .collect();
    (rows.len(), offset, entries, summary)
}
fn result_page(
    result: &scan::ScanResult,
    generation: u64,
    request: &Value,
    comparison: Option<&crate::compare::Comparison>,
) -> Value {
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
    let allocated_available =
        result.nodes[0].allocated.known > 0 || result.nodes[0].allocated.unknown == 0;
    let allocated = allocated_available && request["allocated"].as_bool() == Some(true);
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
    let (total, offset, mut entries) = if mode == "changes" {
        let (total, offset, entries, _) = changes_page(comparison, language, request);
        (total, offset, entries)
    } else if mode == "extensions" {
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
        if allocated && sort == scan::Sort::Size {
            rows.sort_by_key(|index| std::cmp::Reverse(result.nodes[*index].allocated.known));
        }
        let offset = offset.min(rows.len());
        let entries: Vec<_> = rows.iter().skip(offset).take(provider::PAGE_ENTRIES).map(|index| {
            let n = &result.nodes[*index];
            json!({"id":index,"name":n.name,"locator":n.locator,"directory":n.kind == scan::Kind::Directory,
                "size_label":display_measure(language,if allocated {n.allocated} else {n.logical}),"logical_label":display_measure(language,n.logical),"allocated_label":display_measure(language,n.allocated),"hard_link":n.hard_link,
                "complete":n.complete,"error":n.error,"scan_note":if n.kind==scan::Kind::Directory && !n.complete {crate::i18n::text(language,"incomplete_directory")} else {""}})
        }).collect();
        (rows.len(), offset, entries)
    };
    let mut response = json!({"generation":generation,"status":if result.cancelled || result.limited || !result.nodes[0].complete {"partial"} else {"complete"},
        "scope":result.scope,"summary":format!("{} · {} · {}",crate::i18n::counted(language,result.nodes.len() as u64,"object"),display_measure(language,result.nodes[0].logical),crate::i18n::counted(language,result.errors,"error")),
        "directory":directory,"parent":result.nodes[directory].parent,"name":result.nodes[directory].name,
        "offset":offset,"total":total});
    response["allocated_available"] = json!(allocated_available);
    if mode == "changes" {
        response["summary"] = json!(changes_page(comparison, language, request).3);
    }
    if let Some(memory) = &result.memory {
        let summary = response["summary"].as_str().unwrap_or_default();
        response["summary"] = json!(format!(
            "{} · {}",
            summary,
            crate::i18n::memory_diagnostics(language, memory)
        ));
    }
    if let Some(reason) = result.incomplete_reason() {
        let summary = response["summary"].as_str().unwrap_or_default();
        response["summary"] = json!(format!(
            "{} · {}",
            summary,
            crate::i18n::text(language, reason)
        ));
    }
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

/// A saved result chosen in a native document picker. The picker copies only that
/// one file into app-private storage; Rust reads the copy and deletes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenOutcome {
    Ready(PathBuf),
    Cancelled,
    TooLarge,
    Failed,
}
struct OpenRequest {
    id: u64,
    sender: mpsc::SyncSender<OpenOutcome>,
}
fn opens() -> &'static Mutex<Option<OpenRequest>> {
    static VALUE: OnceLock<Mutex<Option<OpenRequest>>> = OnceLock::new();
    VALUE.get_or_init(Mutex::default)
}
pub fn request_open() -> Option<mpsc::Receiver<OpenOutcome>> {
    static SERIAL: AtomicU64 = AtomicU64::new(1);
    let id = SERIAL
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
        .ok()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    {
        let mut pending = opens().lock().unwrap_or_else(|p| p.into_inner());
        if pending.is_some() {
            return None;
        }
        *pending = Some(OpenRequest { id, sender });
    }
    let accepted = request(
        json!({"op":"open_saved", "id":id, "limit":crate::import::Limits::platform().max_bytes}),
    )
    .is_ok_and(|response| response["accepted"].as_bool() == Some(true));
    if !accepted {
        open_complete(id, 0, None);
        return None;
    }
    Some(receiver)
}
/// Called by the platform once the picker finished. Codes: 1 ready, 2 cancelled,
/// 3 too large, anything else failed. Stale or unknown ids are ignored.
pub fn open_complete(id: u64, code: i32, path: Option<&str>) {
    let mut pending = opens().lock().unwrap_or_else(|p| p.into_inner());
    if pending.as_ref().is_some_and(|open| open.id == id) {
        let open = pending.take().expect("matching open");
        drop(pending);
        let valid = path.map(PathBuf::from).filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("spacemosaic-import-"))
        });
        let outcome = match (code, valid) {
            (1, Some(path)) => OpenOutcome::Ready(path),
            (2, _) => OpenOutcome::Cancelled,
            (3, _) => OpenOutcome::TooLarge,
            _ => OpenOutcome::Failed,
        };
        if let (OpenOutcome::Failed | OpenOutcome::TooLarge, Some(path)) = (&outcome, path) {
            let _ = std::fs::remove_file(path);
        }
        let _ = open.sender.try_send(outcome);
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
                listing_complete: None,
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
            limit_reason: None,
            memory: None,
            cross_filesystems: false,
            errors: 0,
            scope_identity: None,
            app_version: None,
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
    fn native_partial_results_explain_limits_and_do_not_label_unknown_directories_as_empty() {
        let mut result = result(3);
        result.limited = true;
        result.limit_reason = Some(scan::LimitReason::MemoryLimit);
        result.nodes[0].complete = false;
        result.nodes[1].kind = scan::Kind::Directory;
        result.nodes[1].complete = false;
        result.nodes[1].logical = scan::Measure {
            unknown: 1,
            ..Default::default()
        };
        for language in crate::i18n::TAGS {
            let page = result_page(&result, 7, &json!({"language":language}), None);
            let index = crate::i18n::TAGS
                .iter()
                .position(|tag| *tag == language)
                .unwrap();
            assert_eq!(page["status"], "partial");
            assert!(
                page["summary"]
                    .as_str()
                    .unwrap()
                    .contains(crate::i18n::text(index, "scan_memory_limit"))
            );
            let directory = page["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["directory"] == true)
                .unwrap();
            assert!(
                directory["size_label"]
                    .as_str()
                    .unwrap()
                    .starts_with(crate::i18n::text(index, "unknown"))
            );
            assert_eq!(
                directory["scan_note"],
                crate::i18n::text(index, "incomplete_directory")
            );
        }
    }
    #[test]
    fn native_results_are_paged_sorted_and_filtered_without_storage_access() {
        let result = result(1000);
        let first = result_page(&result, 7, &json!({"language":"ru"}), None);
        assert_eq!(first["entries"].as_array().unwrap().len(), 32);
        assert_eq!(first["entries"][0]["id"], 1000);
        assert_eq!(first["next"], 32);
        assert_eq!(first["total"], 1000);
        assert_eq!(first["has_more"], true);
        let next = result_page(&result, 7, &json!({"offset":32}), None);
        assert_ne!(next["entries"][0]["id"], first["entries"][0]["id"]);
        let filtered = result_page(
            &result,
            7,
            &json!({"mode":"largest","query":"file-123.dat"}),
            None,
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
        let types = result_page(&result, 7, &json!({"mode":"extensions"}), None);
        assert_eq!(types["entries"][0]["name"], "dat");
        assert!(types["entries"][0]["id"].is_null());
    }
    #[test]
    fn native_page_bytes_are_bounded_and_next_offset_matches_the_returned_page() {
        let mut result = result(100);
        for node in &mut result.nodes[1..] {
            node.locator.push_str(&"x".repeat(40 * 1024));
        }
        let page = result_page(&result, 8, &json!({}), None);
        let count = page["entries"].as_array().unwrap().len();
        assert!(count > 0 && count < provider::PAGE_ENTRIES);
        assert_eq!(page["next"], count);
        assert!(page.to_string().len() <= provider::PAGE_BYTES);
        let next = result_page(&result, 8, &json!({"offset":count}), None);
        assert_ne!(page["entries"][0]["id"], next["entries"][0]["id"]);
    }
    #[test]
    fn native_commands_reject_old_generations_and_old_result_rows_after_rescan() {
        let _session = bridge::SESSION_TEST_LOCK.lock().unwrap();
        let generation = bridge::begin();
        publish(generation, Arc::new(result(2)));
        let accepted: Value = serde_json::from_str(&page(
            &json!({"command":"navigate","index":1,"generation":generation}).to_string(),
        ))
        .unwrap();
        assert_eq!(accepted["accepted"], true);
        let query = "Робин 日本語 Português";
        let response: Value = serde_json::from_str(&page(
            &json!({"command":"search","generation":generation,"query":query}).to_string(),
        ))
        .unwrap();
        assert_eq!(response["accepted"], true);
        assert!(matches!(take_command(), Some(Command::Navigate(1))));
        assert!(matches!(take_command(), Some(Command::Search(actual)) if actual == query));
        page(&json!({"command":"navigate","index":1,"generation":generation}).to_string());
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
        focus(0, false, true, true);
        assert!(
            back(),
            "Android Back must dismiss settings before leaving the app"
        );
        assert!(matches!(take_command(), Some(Command::CloseSettings)));
        focus(0, false, false, true);
        assert!(
            back(),
            "Android Back must leave the result screen before exiting"
        );
        assert!(matches!(take_command(), Some(Command::Back)));
        focus(0, false, false, false);
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
    #[test]
    fn native_memory_inputs_and_separate_scope_reject_invalid_and_stale_commands() {
        let _session = bridge::SESSION_TEST_LOCK.lock().unwrap();
        let generation = bridge::begin();
        publish(generation, Arc::new(result(2)));
        for mib in [0u64, 15, 4097, u64::MAX] {
            assert_eq!(
                serde_json::from_str::<Value>(&page(
                    &json!({"command":"memory_manual","generation":generation,"mib":mib})
                        .to_string()
                ))
                .unwrap()["accepted"],
                false
            );
        }
        page(&json!({"command":"memory_manual","generation":generation,"mib":128}).to_string());
        assert!(matches!(
            take_command(),
            Some(Command::Memory(crate::memory::BudgetMode::Manual {
                mib: 128
            }))
        ));
        let denied =
            page(&json!({"command":"scan_folder","generation":generation,"index":1}).to_string());
        assert_eq!(
            serde_json::from_str::<Value>(&denied).unwrap()["accepted"],
            false
        );
        page(&json!({"command":"scan_folder","generation":generation,"index":0}).to_string());
        assert!(matches!(take_command(), Some(Command::ScanFolder(0))));
        let newer = bridge::begin();
        publish_view(newer, Arc::new(result(2)), true, None);
        let denied =
            page(&json!({"command":"scan_folder","generation":newer,"index":0}).to_string());
        assert_eq!(
            serde_json::from_str::<Value>(&denied).unwrap()["accepted"],
            false
        );
        assert_eq!(
            page(&json!({"command":"memory_automatic","generation":generation}).to_string()),
            "{}"
        );
    }
    #[test]
    fn unavailable_allocation_preserves_known_sizes_in_native_results() {
        let mut result = result(2);
        result.nodes[0].allocated = scan::Measure {
            unknown: 2,
            ..Default::default()
        };
        let page = result_page(&result, 1, &json!({"allocated":true}), None);
        assert_eq!(page["allocated_available"], false);
        for entry in page["entries"].as_array().unwrap() {
            assert_eq!(entry["size_label"], entry["logical_label"]);
            assert_ne!(entry["size_label"], entry["allocated_label"]);
        }
    }

    #[test]
    fn native_changes_page_lists_events_and_saved_results_are_flagged_historical() {
        use crate::compare;
        use std::sync::atomic::AtomicBool;
        let baseline = result(3);
        let mut current = result(3);
        current.nodes[1].logical.known += 5000;
        current.nodes[2].logical.unknown = 1;
        current.nodes[2].logical.known = 0;
        scan::aggregate(&mut current.nodes);
        let comparison = compare::compare(
            Arc::new(baseline),
            Arc::new(current.clone()),
            true,
            &AtomicBool::new(false),
        )
        .unwrap();
        let page = result_page(
            &current,
            3,
            &json!({"mode":"changes","language":"en"}),
            Some(&comparison),
        );
        let entries = page["entries"].as_array().unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e["size_label"].as_str().unwrap().contains("Grew"))
        );
        assert!(
            entries
                .iter()
                .any(|e| e["size_label"].as_str().unwrap().contains("Change unknown"))
        );
        assert!(
            page["summary"]
                .as_str()
                .unwrap()
                .contains("CONFIRMED NET CHANGE")
        );
        let empty = result_page(&current, 3, &json!({"mode":"changes"}), None);
        assert_eq!(empty["total"], 0);
        let _session = bridge::SESSION_TEST_LOCK.lock().unwrap();
        let generation = bridge::begin();
        publish_view(generation, Arc::new(current), true, None);
        let page: Value = serde_json::from_str(&page_request("{}")).unwrap();
        assert_eq!(page["historical"], true);
    }
    fn setup_operand(name: &str, captured: u64, partial: bool) -> SetupOperand {
        SetupOperand {
            name: name.into(),
            live: false,
            captured,
            objects: 12,
            partial,
        }
    }
    fn assessment(verdict: crate::compare::ScopeVerdict) -> crate::compare::ScopeAssessment {
        crate::compare::ScopeAssessment {
            verdict,
            notes: vec![crate::compare::ScopeNote::NoRecordedIdentity],
        }
    }
    #[test]
    fn native_comparison_page_names_files_dates_direction_and_what_may_start() {
        use crate::compare::ScopeVerdict;
        let mut setup = Setup {
            before: Some(setup_operand("pixel-day1.json", 1_790_000_000, false)),
            after: Some(setup_operand("pixel-day2.json", 1_790_259_200, true)),
            assessment: Some(assessment(ScopeVerdict::Matching)),
            ..Setup::default()
        };
        let page = setup_page(0, &setup);
        let lines: Vec<_> = page["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_str().unwrap().to_owned())
            .collect();
        let all = lines.join("\n");
        for expected in [
            "Before: File: pixel-day1.json · 2026-09-21 14:13 UTC",
            "After: File: pixel-day2.json · 2026-09-24 14:13 UTC",
            "Before \u{2192} After",
            crate::i18n::text(0, "cmp_verdict_matching"),
            crate::i18n::text(0, "partial"),
        ] {
            assert!(all.contains(expected), "{expected} in {all}");
        }
        assert_eq!(page["can_start"], true);
        assert_eq!(page["needs_confirmation"], false);
        assert_eq!(page["can_swap"], true);
        // Not enough recorded: starting needs the person's confirmation.
        setup.assessment = Some(assessment(ScopeVerdict::Insufficient));
        let page = setup_page(0, &setup);
        assert_eq!(
            (
                page["can_start"].clone(),
                page["needs_confirmation"].clone()
            ),
            (json!(false), json!(true))
        );
        setup.confirmed = true;
        assert_eq!(setup_page(0, &setup)["can_start"], true);
        // Busy work and incompatible areas never start.
        setup.comparing = true;
        assert_eq!(setup_page(0, &setup)["can_start"], false);
        setup.comparing = false;
        setup.assessment = Some(assessment(ScopeVerdict::Incompatible));
        assert_eq!(setup_page(0, &setup)["can_start"], false);
        // A side that is not chosen is said so, and nothing can be started or swapped.
        setup.after = None;
        setup.assessment = None;
        let page = setup_page(1, &setup);
        assert_eq!(
            (page["can_start"].clone(), page["can_swap"].clone()),
            (json!(false), json!(false))
        );
        let text = page["lines"].to_string();
        assert!(
            text.contains(crate::i18n::text(1, "compare_not_chosen")),
            "{text}"
        );
        assert!(
            text.contains(crate::i18n::text(1, "compare_needs_two")),
            "{text}"
        );
        // A comparison that already exists is not started again.
        setup.finished = true;
        setup.assessment = Some(assessment(ScopeVerdict::Matching));
        setup.after = Some(setup_operand("b.json", 1, false));
        assert_eq!(setup_page(0, &setup)["can_start"], false);
    }

    #[test]
    fn native_comparison_commands_are_accepted_for_the_current_generation_only() {
        let _session = bridge::SESSION_TEST_LOCK.lock().unwrap();
        let generation = bridge::begin();
        publish_view(generation, Arc::new(result(2)), true, None);
        publish_setup(
            generation,
            Setup {
                before: Some(setup_operand("a.json", 1, false)),
                ..Setup::default()
            },
        );
        let page: Value = serde_json::from_str(&page(
            r#"{"mode":"changes","language":"ru","directory":0,"offset":0}"#,
        ))
        .unwrap();
        assert!(
            page["compare"]["lines"].to_string().contains("До"),
            "{page}"
        );
        assert_eq!(page["compare"]["can_show_after"], false);
        while take_command().is_some() {}
        for (name, index, expected) in [
            ("open_before", 0, "OpenFor(Baseline)"),
            ("open_after", 0, "OpenFor(Current)"),
            ("swap_direction", 0, "SwapDirection"),
            ("confirm_scope", 1, "ConfirmScope(true)"),
            ("start_compare", 0, "StartCompare"),
            ("show_before", 0, "ShowSide(Baseline)"),
            ("show_after", 0, "ShowSide(Current)"),
        ] {
            let request = json!({"command":name,"index":index,"generation":generation});
            let response: Value =
                serde_json::from_str(&page_request(&request.to_string())).unwrap();
            assert_eq!(response["accepted"], true, "{name}");
            let command = take_command().expect(name);
            let rendered = match command {
                Command::OpenFor(side) => format!("OpenFor({side:?})"),
                Command::SwapDirection => "SwapDirection".into(),
                Command::ConfirmScope(on) => format!("ConfirmScope({on})"),
                Command::StartCompare => "StartCompare".into(),
                Command::ShowSide(side) => format!("ShowSide({side:?})"),
                _ => "other".into(),
            };
            assert_eq!(rendered, expected);
        }
        let stale = json!({"command":"swap_direction","generation":generation + 1000});
        assert_eq!(page_request(&stale.to_string()), "{}");
        assert!(take_command().is_none());
    }
    fn page_request(request: &str) -> String {
        page(request)
    }
    #[test]
    fn native_open_requests_accept_only_matching_ids_and_private_copies() {
        let _session = bridge::SESSION_TEST_LOCK.lock().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        *opens().lock().unwrap() = Some(OpenRequest { id: 77, sender });
        open_complete(76, 1, Some("/tmp/spacemosaic-import-76.json"));
        assert!(receiver.try_recv().is_err(), "a stale id changes nothing");
        // A path outside the app's naming scheme is never trusted.
        open_complete(77, 1, Some("/etc/passwd"));
        assert_eq!(receiver.try_recv().unwrap(), OpenOutcome::Failed);
        assert!(opens().lock().unwrap().is_none());
        for (code, expected) in [(2, OpenOutcome::Cancelled), (3, OpenOutcome::TooLarge)] {
            let (sender, receiver) = mpsc::sync_channel(1);
            *opens().lock().unwrap() = Some(OpenRequest { id: 78, sender });
            open_complete(78, code, None);
            assert_eq!(receiver.try_recv().unwrap(), expected);
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        *opens().lock().unwrap() = Some(OpenRequest { id: 79, sender });
        open_complete(79, 1, Some("/tmp/spacemosaic-import-79.json"));
        assert_eq!(
            receiver.try_recv().unwrap(),
            OpenOutcome::Ready(PathBuf::from("/tmp/spacemosaic-import-79.json"))
        );
    }
}
