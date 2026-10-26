//! Bounded, generation-tagged messages from native metadata providers.
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock};

pub const MAX_ENTRIES: usize = 32;
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ready,
    Cancelled,
    Denied,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    /// Provider URI on Android, security-scoped file URL on iOS. Never a fake path.
    pub locator: String,
    pub directory: bool,
    pub logical_bytes: Option<u64>,
    pub allocated_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    pub generation: u64,
    pub status: Status,
    pub scope: Option<String>,
    pub entries: Vec<Entry>,
    pub truncated: bool,
    pub persisted: bool,
    /// Stable diagnostic code. UI supplies localized wording.
    pub error: Option<String>,
}

impl Listing {
    pub fn parse(json: &[u8]) -> Result<Self, &'static str> {
        if json.len() > MAX_MESSAGE_BYTES {
            return Err("message_too_large");
        }
        let listing: Self = serde_json::from_slice(json).map_err(|_| "invalid_message")?;
        if listing.entries.len() > MAX_ENTRIES {
            return Err("too_many_entries");
        }
        if listing.status == Status::Ready && listing.scope.is_none() {
            return Err("missing_scope");
        }
        Ok(listing)
    }

    /// None means overflow, not a wrapped or saturated total.
    pub fn known_logical_bytes(&self) -> Option<u64> {
        self.entries
            .iter()
            .filter(|e| !e.directory)
            .try_fold(0_u64, |sum, e| {
                sum.checked_add(e.logical_bytes.unwrap_or(0))
            })
    }

    pub fn unknown_file_sizes(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| !e.directory && e.logical_bytes.is_none())
            .count()
    }
}

#[derive(Default)]
struct Mailbox {
    generation: u64,
    pending: Option<Listing>,
    completed: bool,
}

impl Mailbox {
    fn begin(&mut self) -> u64 {
        // Never reuse an ID. Exhaustion is unreachable for interactive use.
        self.generation = self
            .generation
            .checked_add(1)
            .expect("generation exhausted");
        self.pending = None;
        self.completed = false;
        self.generation
    }

    fn publish(&mut self, listing: Listing) -> bool {
        if listing.generation != self.generation || self.completed {
            return false;
        }
        self.pending = Some(listing);
        self.completed = true;
        true
    }
}

fn mailbox() -> &'static Mutex<Mailbox> {
    static MAILBOX: OnceLock<Mutex<Mailbox>> = OnceLock::new();
    MAILBOX.get_or_init(Mutex::default)
}

pub fn begin() -> u64 {
    let generation = mailbox().lock().unwrap_or_else(|p| p.into_inner()).begin();
    crate::mobile::clear(generation);
    wake();
    generation
}
pub fn generation() -> u64 {
    mailbox()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .generation
}
pub fn wake() {
    if let Some(wake) = waker().lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
        wake();
    }
}

pub fn is_current(generation: u64) -> bool {
    mailbox()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .generation
        == generation
}

pub fn receive(json: &[u8]) -> Result<bool, &'static str> {
    let listing = Listing::parse(json)?;
    let accepted = mailbox()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .publish(listing);
    if accepted && let Some(wake) = waker().lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
        wake();
    }
    Ok(accepted)
}

pub fn take() -> Option<Listing> {
    mailbox()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .pending
        .take()
}

type Wake = Arc<dyn Fn() + Send + Sync>;
fn waker() -> &'static Mutex<Option<Wake>> {
    static WAKER: OnceLock<Mutex<Option<Wake>>> = OnceLock::new();
    WAKER.get_or_init(|| Mutex::new(None))
}
pub fn set_waker(wake: Wake) {
    *waker().lock().unwrap_or_else(|p| p.into_inner()) = Some(wake);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(generation: u64) -> Listing {
        Listing {
            generation,
            status: Status::Ready,
            scope: Some("content://provider/tree/a".into()),
            entries: vec![Entry {
                name: "unknown".into(),
                locator: "content://provider/document/a".into(),
                directory: false,
                logical_bytes: None,
                allocated_bytes: None,
            }],
            truncated: false,
            persisted: false,
            error: None,
        }
    }

    #[test]
    fn unknown_is_preserved_and_not_reported_as_a_known_zero() {
        let value = listing(1);
        let parsed = Listing::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(parsed, value);
        assert_eq!(parsed.unknown_file_sizes(), 1);
        assert_eq!(parsed.entries[0].logical_bytes, None);
    }

    #[test]
    fn old_results_and_duplicates_do_not_replace_current_results() {
        let mut mailbox = Mailbox::default();
        let old = mailbox.begin();
        let current = mailbox.begin();
        assert!(!mailbox.publish(listing(old)));
        assert!(mailbox.publish(listing(current)));
        assert!(!mailbox.publish(listing(current)));
        assert_eq!(mailbox.pending.take().unwrap().generation, current);
        assert!(!mailbox.publish(listing(current)));
    }

    #[test]
    fn cancelled_picker_and_denied_access_are_distinct() {
        let mut value = listing(1);
        value.entries.clear();
        value.scope = None;
        for status in [Status::Cancelled, Status::Denied, Status::Unavailable] {
            value.status = status;
            assert!(Listing::parse(&serde_json::to_vec(&value).unwrap()).is_ok());
        }
        value.status = Status::Ready;
        assert_eq!(
            Listing::parse(&serde_json::to_vec(&value).unwrap()),
            Err("missing_scope")
        );
    }

    #[test]
    fn bounded_payload_and_entry_count() {
        assert_eq!(
            Listing::parse(&vec![b' '; MAX_MESSAGE_BYTES + 1]),
            Err("message_too_large")
        );
        let mut value = listing(1);
        value.entries = vec![value.entries[0].clone(); MAX_ENTRIES + 1];
        assert_eq!(
            Listing::parse(&serde_json::to_vec(&value).unwrap()),
            Err("too_many_entries")
        );
    }

    #[test]
    fn overflow_is_explicit_and_directory_sizes_are_not_file_totals() {
        let mut value = listing(1);
        value.entries[0].logical_bytes = Some(u64::MAX);
        let mut second = value.entries[0].clone();
        second.logical_bytes = Some(1);
        value.entries.push(second);
        assert_eq!(value.known_logical_bytes(), None);
        value.entries[1].directory = true;
        assert_eq!(value.known_logical_bytes(), Some(u64::MAX));
    }
}
