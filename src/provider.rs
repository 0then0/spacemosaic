//! Bounded metadata pages from native storage APIs. No URI is converted to a path.
use crate::scan::{Kind, Measure, Node, Storage};
use serde::Deserialize;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub const PAGE_ENTRIES: usize = 32;
pub const PAGE_BYTES: usize = 256 * 1024;
pub type Transport = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    locator: String,
    kind: Kind,
    logical_bytes: Option<u64>,
    allocated_bytes: Option<u64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    hard_link: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    entries: Vec<Entry>,
    done: bool,
    error: Option<String>,
}
fn measure(value: Option<u64>) -> Measure {
    value
        .map(|known| Measure {
            known,
            ..Default::default()
        })
        .unwrap_or(Measure {
            unknown: 1,
            ..Default::default()
        })
}

pub struct Provider {
    root: String,
    display_name: Option<String>,
    generation: u64,
    scan_id: u64,
    transport: Transport,
    pending: VecDeque<Node>,
    done: bool,
    error: Option<String>,
}
impl Provider {
    pub fn new(root: String, generation: u64, transport: Transport) -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(1);
        let scan_id = SERIAL
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .expect("provider session IDs exhausted");
        Self {
            root,
            display_name: None,
            generation,
            scan_id,
            transport,
            pending: VecDeque::new(),
            done: false,
            error: None,
        }
    }
    /// Names are presentation metadata; the native URI remains unchanged.
    pub fn with_display_name(mut self, display_name: Option<String>) -> Self {
        self.display_name = display_name.filter(|name| !name.trim().is_empty());
        self
    }
    fn request(&self, operation: &str, locator: Option<&str>) -> Result<Page, String> {
        let request =
            serde_json::json!({"op": operation, "generation": self.generation, "scan":self.scan_id, "locator": locator})
                .to_string();
        let response = (self.transport)(&request)?;
        if response.len() > PAGE_BYTES {
            return Err("message_too_large".into());
        }
        let page: Page =
            serde_json::from_str(&response).map_err(|_| "metadata_unavailable".to_owned())?;
        if page.entries.len() > PAGE_ENTRIES
            || (!page.done && page.entries.is_empty() && operation == "next")
        {
            return Err("metadata_unavailable".into());
        }
        Ok(page)
    }
}
impl Storage for Provider {
    fn root(&self) -> Node {
        Node {
            parent: None,
            name: self.display_name.clone().unwrap_or_else(|| {
                self.root
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&self.root)
                    .to_owned()
            }),
            locator: self.root.clone(),
            native_locator: self.root.clone(),
            kind: Kind::Directory,
            logical: Measure::default(),
            allocated: Measure::default(),
            complete: false,
            hard_link: false,
            error: None,
            path: None,
        }
    }
    fn open_directory(&mut self, locator: &str) -> Result<(), String> {
        self.pending.clear();
        self.error = None;
        self.done = false;
        let page = self.request("open", Some(locator))?;
        if !page.entries.is_empty() {
            return Err("metadata_unavailable".into());
        }
        if let Some(error) = page.error {
            Err(error)
        } else {
            Ok(())
        }
    }
    fn next_entry(&mut self) -> Result<Option<Node>, String> {
        if self.pending.is_empty() && !self.done {
            let page = self.request("next", None)?;
            self.done = page.done || page.error.is_some();
            self.error = page.error;
            for entry in page.entries {
                if entry.locator.is_empty() || entry.name.is_empty() {
                    return Err("metadata_unavailable".into());
                }
                self.pending.push_back(Node {
                    parent: None,
                    name: entry.name,
                    native_locator: entry.locator.clone(),
                    locator: entry.locator,
                    kind: entry.kind,
                    logical: measure(entry.logical_bytes),
                    allocated: measure(entry.allocated_bytes),
                    complete: entry.kind != Kind::Directory && entry.error.is_none(),
                    hard_link: entry.hard_link,
                    error: entry.error,
                    path: None,
                });
            }
        }
        if let Some(node) = self.pending.pop_front() {
            Ok(Some(node))
        } else if let Some(error) = self.error.take() {
            Err(error)
        } else {
            Ok(None)
        }
    }
    fn close_directory(&mut self) {
        let _ = self.request("close", None);
    }
    fn interrupt(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let transport = self.transport.clone();
        let generation = self.generation;
        let scan_id = self.scan_id;
        Some(Arc::new(move || {
            let _ = transport(
                &serde_json::json!({"op":"cancel", "generation":generation, "scan":scan_id})
                    .to_string(),
            );
        }))
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        let _ = self.request("finish", None);
    }
}
