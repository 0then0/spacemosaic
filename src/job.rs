//! Cancellable background work for opening and comparing saved results. Only one
//! heavy job runs at a time, so two parsed documents and a diff never coexist
//! beyond the budgets documented for a single operation. A superseded job is
//! cancelled and its receiver dropped: it cannot deliver a result to the UI.
use crate::{
    compare::{self, CompareError, Comparison},
    import::{self, ImportError, Limits},
    scan::ScanResult,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
};

static HEAVY: Mutex<()> = Mutex::new(());
static TICKETS: AtomicU64 = AtomicU64::new(1);

pub struct Job<T> {
    /// Unique across the process; lets callers discard anything not from the latest job.
    pub ticket: u64,
    /// Bytes read so far for an import.
    pub progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    receiver: Receiver<T>,
}
impl<T: Send + 'static> Job<T> {
    fn spawn(
        work: impl FnOnce(&AtomicBool, &AtomicU64) -> T + Send + 'static,
        cancelled: impl FnOnce() -> T + Send + 'static,
    ) -> Self {
        let ticket = TICKETS.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU64::new(0));
        let (sender, receiver) = mpsc::sync_channel(1);
        let (flag, counter) = (cancel.clone(), progress.clone());
        let spawned = std::thread::Builder::new()
            .name("saved-result".into())
            .spawn(move || {
                // Poisoning cannot leave shared state inconsistent: the lock guards nothing.
                let _turn = HEAVY.lock().unwrap_or_else(|p| p.into_inner());
                let value = if flag.load(Ordering::Relaxed) {
                    cancelled()
                } else {
                    work(&flag, &counter)
                };
                let _ = sender.send(value);
                crate::bridge::wake();
            });
        debug_assert!(spawned.is_ok(), "cannot start the saved-result worker");
        Self {
            ticket,
            progress,
            cancel,
            receiver,
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn take(&self) -> Option<T> {
        self.receiver.try_recv().ok()
    }
}
impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub type OpenJob = Job<Result<ScanResult, ImportError>>;
pub type CompareJob = Job<Result<Comparison, CompareError>>;

/// Read a saved result. The file is only read; its contents never cause any
/// access to the location it describes. `delete_after` removes an app-private
/// copy made by a native document picker once the import has finished.
pub fn open(path: PathBuf, limits: Limits, delete_after: bool) -> OpenJob {
    open_with(path, move || limits, delete_after)
}

/// Sample local memory after the worker acquires the heavy-job lock so that
/// allocations from a superseded import do not affect the next open. `held` is
/// the estimate of what the session already retains; see `memory::operation_budget`.
pub fn open_local(path: PathBuf, delete_after: bool, held: usize) -> OpenJob {
    open_with(path, move || Limits::for_device_holding(held), delete_after)
}

fn open_with(
    path: PathBuf,
    limits: impl FnOnce() -> Limits + Send + 'static,
    delete_after: bool,
) -> OpenJob {
    let leftover = path.clone();
    Job::spawn(
        move |cancel, progress| {
            let outcome = import::import_file(&path, limits(), cancel, progress);
            if delete_after {
                let _ = std::fs::remove_file(&path);
            }
            outcome
        },
        move || {
            if delete_after {
                let _ = std::fs::remove_file(&leftover);
            }
            Err(ImportError::Cancelled)
        },
    )
}

/// Compare with a memory allowance sampled when the worker starts, after any
/// older heavy job has released its memory. `held` is what the session retains,
/// including both operands.
pub fn compare_local(
    baseline: Arc<ScanResult>,
    current: Arc<ScanResult>,
    confirm_scope: bool,
    held: usize,
) -> CompareJob {
    compare_with(baseline, current, confirm_scope, move || {
        crate::memory::operation_budget(held)
    })
}

/// Compare with an explicit allowance of additional bytes.
pub fn compare(
    baseline: Arc<ScanResult>,
    current: Arc<ScanResult>,
    confirm_scope: bool,
    limit: usize,
) -> CompareJob {
    compare_with(baseline, current, confirm_scope, move || limit)
}

fn compare_with(
    baseline: Arc<ScanResult>,
    current: Arc<ScanResult>,
    confirm_scope: bool,
    limit: impl FnOnce() -> usize + Send + 'static,
) -> CompareJob {
    Job::spawn(
        move |cancel, _| compare::compare_within(baseline, current, confirm_scope, cancel, limit()),
        || Err(CompareError::Cancelled),
    )
}
