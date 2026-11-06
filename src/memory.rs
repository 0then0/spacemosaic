//! Memory admission policy and bounded-cost scan monitoring. Values are heuristics, not OOM guarantees.
//! Platform readings are optional and never substituted with zero on failure.
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
mod platform;
pub const MIB: u64 = 1024 * 1024;
pub const MIN_MANUAL_MIB: u32 = 16;
pub const MAX_MANUAL_MIB: u32 = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum BudgetMode {
    #[default]
    Automatic,
    Manual {
        mib: u32,
    },
}
impl BudgetMode {
    pub fn valid(self) -> bool {
        match self {
            Self::Automatic => true,
            Self::Manual { mib } => (MIN_MANUAL_MIB..=MAX_MANUAL_MIB).contains(&mib),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Desktop,
    Android,
    Ios,
}
impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "android") {
            Self::Android
        } else if cfg!(target_os = "ios") {
            Self::Ios
        } else {
            Self::Desktop
        }
    }
    fn mobile(self) -> bool {
        self != Self::Desktop
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessMetric {
    Rss,
    PhysicalFootprint,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub physical: Option<u64>,
    pub available: Option<u64>,
    /// RSS (desktop/Android), physical footprint (iOS). Compare only like samples.
    pub process: Option<u64>,
    pub process_metric: Option<ProcessMetric>,
    /// Additional process allocation permitted by an OS limit, NOT total RAM.
    pub process_headroom: Option<u64>,
    /// Android managed-heap memory class: a sizing hint, NOT a native heap limit.
    pub recommendation: Option<u64>,
    /// Remaining shared cgroup/job allowance, NOT a per-process limit.
    pub group_headroom: Option<u64>,
    pub group_limit: Option<u64>,
    pub critical: bool,
}
impl Snapshot {
    pub fn sanitized(mut self) -> Self {
        self.physical = self.physical.filter(|v| *v >= MIB);
        if self
            .available
            .zip(self.physical)
            .is_some_and(|(a, p)| a > p)
        {
            self.available = None;
        }
        self.recommendation = self.recommendation.filter(|v| *v >= MIB);
        self
    }
}
fn lower(value: &mut u64, bound: Option<u64>) {
    if let Some(bound) = bound {
        *value = (*value).min(bound);
    }
}
fn system_reserve(mobile: bool, physical: Option<u64>) -> u64 {
    (if mobile { 256 * MIB } else { 512 * MIB }).max(physical.unwrap_or(0) / 16)
}
fn group_reserve(mobile: bool) -> u64 {
    if mobile { 32 * MIB } else { 64 * MIB }
}
fn process_reserve(mobile: bool) -> u64 {
    if mobile { 64 * MIB } else { 128 * MIB }
}
/// Additional scanner allocations. Explicit reserves cover other app work;
/// division leaves room for indexes, sorting, allocator overhead and concurrency.
pub fn choose(mode: BudgetMode, platform: Platform, readings: Snapshot) -> u64 {
    let s = readings.sanitized();
    let mobile = platform.mobile();
    let fallback = if mobile { 32 * MIB } else { 128 * MIB };
    let mut auto = s
        .physical
        .map(|p| {
            (p / if mobile { 32 } else { 16 }).min(if mobile { 128 * MIB } else { 2048 * MIB })
        })
        .unwrap_or(fallback);
    lower(
        &mut auto,
        s.available.map(|a| {
            a.saturating_sub(system_reserve(mobile, s.physical)) / if mobile { 6 } else { 4 }
        }),
    );
    lower(&mut auto, s.group_limit.map(|l| l / 16));
    lower(
        &mut auto,
        s.group_headroom
            .map(|a| a.saturating_sub(group_reserve(mobile)) / 3),
    );
    lower(
        &mut auto,
        s.process_headroom
            .map(|a| a.saturating_sub(process_reserve(mobile)) / 3),
    );
    if platform == Platform::Android {
        lower(&mut auto, s.recommendation.map(|v| v / 4));
    }
    match mode {
        BudgetMode::Manual { mib } if mode.valid() => u64::from(mib) * MIB,
        _ => auto,
    }
}

/// Additional allocations for parsing, validating and opening one saved result.
pub(crate) fn import_budget() -> usize {
    let (sample, unmeasured_group_limit) = platform::sample_for_import();
    usize::try_from(choose_import(
        Platform::current(),
        sample,
        unmeasured_group_limit,
    ))
    .unwrap_or(usize::MAX)
}

fn choose_import(
    platform: Platform,
    readings: Snapshot,
    unmeasured_group_limit: Option<u64>,
) -> u64 {
    let s = readings.sanitized();
    if s.critical {
        return 0;
    }
    let mobile = platform.mobile();
    let has_available =
        s.available.is_some() || (platform == Platform::Ios && s.process_headroom.is_some());
    let mut budget = if has_available {
        s.physical
            .map_or(u64::MAX, |p| p / if mobile { 16 } else { 8 })
    } else {
        let mut fallback = s
            .physical
            .map_or(if mobile { 64 * MIB } else { 256 * MIB }, |p| {
                (p / if mobile { 32 } else { 16 }).min(if mobile { 128 * MIB } else { 512 * MIB })
            });
        if platform == Platform::Android {
            lower(&mut fallback, s.recommendation.map(|v| v / 4));
        }
        // Keep a bounded process allowance for the old result, the import and
        // the rest of the app when there is no measured remaining headroom.
        let footprint = 2 * fallback;
        fallback.min(footprint.saturating_sub(s.process.unwrap_or(0)))
    };
    // These readings already reflect allocations retained by the current view.
    // A desktop process or group limit alone cannot establish available RAM.
    let remaining = [
        s.available
            .map(|a| a.saturating_sub(system_reserve(mobile, s.physical))),
        s.group_headroom
            .map(|a| a.saturating_sub(group_reserve(mobile))),
        s.process_headroom
            .map(|a| a.saturating_sub(process_reserve(mobile))),
    ]
    .into_iter()
    .flatten()
    .min();
    // The import estimate includes the validation and index peak, so a
    // larger share can go to import than to scanner data alone.
    lower(
        &mut budget,
        remaining.map(|a| a / if mobile { 3 } else { 2 }),
    );
    // Known headroom for one cgroup does not cover another cgroup whose
    // usage could not be sampled. Windows job usage can also be unavailable.
    let group_limit =
        unmeasured_group_limit.or(s.group_limit.filter(|_| s.group_headroom.is_none()));
    lower(&mut budget, group_limit.map(|l| l / 16));
    budget
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    pub budget_bytes: u64,
    pub estimated_scanner_bytes: u64,
    pub baseline_process_bytes: Option<u64>,
    #[serde(default)]
    pub baseline_process_metric: Option<ProcessMetric>,
    pub last: Snapshot,
}

struct Listener {
    id: u64,
    flag: Weak<AtomicBool>,
    interrupt: Option<Arc<dyn Fn() + Send + Sync>>,
}
fn listeners() -> &'static Mutex<Vec<Listener>> {
    static LISTENERS: OnceLock<Mutex<Vec<Listener>>> = OnceLock::new();
    LISTENERS.get_or_init(Mutex::default)
}
/// Native callbacks carry a scan-specific ID. A queued event cannot target a later scan.
pub fn pressure(id: u64) {
    let interrupt = {
        let state = listeners().lock().unwrap_or_else(|p| p.into_inner());
        let Some(listener) = state.iter().find(|l| l.id == id) else {
            return;
        };
        let Some(flag) = listener.flag.upgrade() else {
            return;
        };
        if flag.swap(true, Ordering::Relaxed) {
            return;
        }
        listener.interrupt.clone()
    };
    if let Some(interrupt) = interrupt {
        interrupt();
    }
}

pub(crate) struct Monitor {
    id: u64,
    flag: Arc<AtomicBool>,
    subscription: platform::Subscription,
    pub diagnostics: Diagnostics,
    platform: Platform,
    sampled: Instant,
    observed_scanner_bytes: u64,
    scanner_bytes_at_sample: u64,
    native: bool,
}
impl Monitor {
    pub fn new(
        mode: BudgetMode,
        ceiling: usize,
        interrupt: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        Self::start(mode, ceiling, interrupt, Platform::current(), None)
    }
    fn start(
        mode: BudgetMode,
        ceiling: usize,
        interrupt: Option<Arc<dyn Fn() + Send + Sync>>,
        platform: Platform,
        injected: Option<Snapshot>,
    ) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .expect("memory monitor IDs exhausted");
        let flag = Arc::new(AtomicBool::new(false));
        listeners()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Listener {
                id,
                flag: Arc::downgrade(&flag),
                interrupt,
            });
        let native = injected.is_none();
        let subscription = if native { platform::subscribe(id) } else { 0 };
        let s = injected.unwrap_or_else(platform::sample).sanitized();
        let diagnostics = Diagnostics {
            budget_bytes: choose(mode, platform, s).min(ceiling as u64),
            baseline_process_bytes: s.process,
            baseline_process_metric: s.process_metric,
            last: s,
            ..Default::default()
        };
        Self {
            id,
            flag,
            subscription,
            diagnostics,
            platform,
            sampled: Instant::now(),
            observed_scanner_bytes: 0,
            scanner_bytes_at_sample: 0,
            native,
        }
    }
    #[cfg(test)]
    pub(crate) fn simulated(
        mode: BudgetMode,
        s: Snapshot,
        interrupt: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        Self::start(mode, usize::MAX, interrupt, Platform::Desktop, Some(s))
    }
    #[cfg(test)]
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    fn accept_sample(&mut self, snapshot: Snapshot) {
        self.diagnostics.last = snapshot.sanitized();
        self.scanner_bytes_at_sample = self.observed_scanner_bytes;
        self.sampled = Instant::now();
    }
    pub fn check(&mut self, estimated: u64) -> Option<crate::scan::LimitReason> {
        self.observed_scanner_bytes = estimated;
        if self.native && self.sampled.elapsed() >= Duration::from_millis(500) {
            self.accept_sample(platform::sample());
        }
        self.reason(estimated)
    }
    pub fn check_pressure(&mut self) -> Option<crate::scan::LimitReason> {
        self.check(self.observed_scanner_bytes)
    }
    pub fn check_growth(
        &mut self,
        current: u64,
        proposed: u64,
    ) -> Option<crate::scan::LimitReason> {
        // A new OS sample describes retained allocations, never the proposed
        // capacity that has not yet been allocated.
        if let Some(reason) = self.check(current).or_else(|| self.reason(proposed)) {
            return Some(reason);
        }
        let reserve = if self.platform.mobile() {
            32 * MIB
        } else {
            64 * MIB
        };
        let additional = proposed.saturating_sub(self.scanner_bytes_at_sample);
        // Prospective allocation bytes versus remaining allowance. This is an
        // admission heuristic, not a comparison of tree bytes with absolute RSS.
        // Charge every allocation since that sample, rather than repeatedly
        // spending the same remaining allowance on individual small growths.
        let s = self.diagnostics.last;
        [s.available, s.group_headroom, s.process_headroom]
            .into_iter()
            .flatten()
            .any(|remaining| additional.saturating_add(reserve) > remaining)
            .then_some(crate::scan::LimitReason::MemoryPressure)
    }
    fn reason(&self, estimated: u64) -> Option<crate::scan::LimitReason> {
        use crate::scan::LimitReason::*;
        let d = &self.diagnostics;
        let s = d.last;
        let reserve = if self.platform.mobile() {
            32 * MIB
        } else {
            64 * MIB
        };
        if self.flag.load(Ordering::Relaxed)
            || s.critical
            || s.available.is_some_and(|v| v < reserve)
            || s.group_headroom.is_some_and(|v| v < reserve)
            || s.process_headroom.is_some_and(|v| v < reserve)
        {
            return Some(MemoryPressure);
        }
        // Same process metric at start and now. Allow UI/index growth in addition
        // to scanner allocations, but never compare an absolute RSS with the tree budget.
        if s.process_metric.is_some()
            && s.process_metric == d.baseline_process_metric
            && s.process
                .zip(d.baseline_process_bytes)
                .is_some_and(|(now, base)| {
                    now.saturating_sub(base)
                        > d.budget_bytes.saturating_mul(2).saturating_add(reserve)
                })
        {
            return Some(ProcessMemoryLimit);
        }
        (estimated > d.budget_bytes).then_some(MemoryLimit)
    }
    pub fn record(&mut self, bytes: u64) {
        self.diagnostics.estimated_scanner_bytes = bytes;
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        listeners()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|l| l.id != self.id);
        if self.native {
            platform::unsubscribe(self.id, &mut self.subscription);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_uses_the_tightest_remaining_allowance() {
        let s = Snapshot {
            physical: Some(8 * 1024 * MIB),
            available: Some(2 * 1024 * MIB),
            ..Snapshot::default()
        };
        assert_eq!(choose_import(Platform::Desktop, s, None), 768 * MIB);
        assert_eq!(
            choose_import(
                Platform::Desktop,
                Snapshot {
                    group_limit: Some(1200 * MIB),
                    group_headroom: Some(1024 * MIB),
                    process: Some(1024 * MIB),
                    ..s
                },
                None,
            ),
            480 * MIB,
            "remaining allowance already accounts for process memory"
        );
        assert_eq!(
            choose_import(
                Platform::Ios,
                Snapshot {
                    available: None,
                    process_headroom: Some(1024 * MIB),
                    ..s
                },
                None,
            ),
            320 * MIB
        );
        for s in [
            Snapshot {
                process_headroom: Some(0),
                ..s
            },
            Snapshot {
                critical: true,
                ..s
            },
        ] {
            assert_eq!(choose_import(Platform::Desktop, s, None), 0);
        }
    }

    #[test]
    fn import_fallback_limits_peak_with_existing_allocations() {
        assert_eq!(
            choose_import(Platform::Desktop, Snapshot::default(), None),
            256 * MIB
        );
        assert_eq!(
            choose_import(Platform::Android, Snapshot::default(), None),
            64 * MIB
        );
        let s = Snapshot {
            physical: Some(8 * 1024 * MIB),
            ..Snapshot::default()
        };
        assert_eq!(choose_import(Platform::Desktop, s, None), 512 * MIB);
        assert_eq!(
            choose_import(
                Platform::Desktop,
                Snapshot {
                    process_headroom: Some(4 * 1024 * MIB),
                    ..s
                },
                None,
            ),
            512 * MIB,
            "virtual address headroom alone cannot replace the fallback"
        );
        assert_eq!(
            choose_import(
                Platform::Desktop,
                Snapshot {
                    process: Some(768 * MIB),
                    ..s
                },
                None,
            ),
            256 * MIB
        );
    }

    #[test]
    fn policy_inputs_and_overflow() {
        let s = Snapshot {
            physical: Some(8 * 1024 * MIB),
            available: Some(4 * 1024 * MIB),
            ..Default::default()
        };
        assert_eq!(
            choose(BudgetMode::Automatic, Platform::Desktop, s),
            512 * MIB
        );
        assert_eq!(choose(BudgetMode::Automatic, Platform::Ios, s), 128 * MIB);
        assert_eq!(
            choose(
                BudgetMode::Automatic,
                Platform::Android,
                Snapshot {
                    recommendation: Some(256 * MIB),
                    ..s
                }
            ),
            64 * MIB
        );
        for platform in [Platform::Desktop, Platform::Android, Platform::Ios] {
            assert!(
                choose(
                    BudgetMode::Automatic,
                    platform,
                    Snapshot {
                        process_headroom: Some(200 * MIB),
                        ..s
                    }
                ) <= 136 * MIB / 3
            );
            assert_eq!(
                choose(
                    BudgetMode::Automatic,
                    platform,
                    Snapshot {
                        group_headroom: Some(0),
                        ..s
                    }
                ),
                0
            );
            assert!(
                choose(
                    BudgetMode::Automatic,
                    platform,
                    Snapshot {
                        physical: Some(u64::MAX),
                        available: Some(u64::MAX),
                        ..Default::default()
                    }
                ) <= 2048 * MIB
            );
        }
        assert_eq!(
            choose(
                BudgetMode::Automatic,
                Platform::Desktop,
                Snapshot::default()
            ),
            128 * MIB
        );
        assert_eq!(
            choose(BudgetMode::Automatic, Platform::Ios, Snapshot::default()),
            32 * MIB
        );
        assert_eq!(
            Snapshot {
                physical: Some(1),
                ..s
            }
            .sanitized()
            .physical,
            None
        );
        assert_eq!(
            Snapshot {
                available: Some(u64::MAX),
                ..s
            }
            .sanitized()
            .available,
            None
        );
        assert_eq!(
            choose(
                BudgetMode::Manual { mib: 256 },
                Platform::Ios,
                Snapshot {
                    critical: true,
                    ..s
                }
            ),
            256 * MIB
        );
        for mib in [0, 15, 4097, u32::MAX] {
            assert!(!BudgetMode::Manual { mib }.valid());
        }
    }
    #[test]
    fn sampled_headroom_cannot_be_spent_again_by_the_next_growth() {
        for snapshot in [
            Snapshot {
                available: Some(100 * MIB),
                ..Default::default()
            },
            Snapshot {
                process_headroom: Some(100 * MIB),
                ..Default::default()
            },
            Snapshot {
                group_headroom: Some(100 * MIB),
                ..Default::default()
            },
        ] {
            let mut monitor = Monitor::simulated(BudgetMode::Manual { mib: 128 }, snapshot, None);
            assert_eq!(monitor.check_growth(0, 24 * MIB), None);
            assert_eq!(monitor.check(24 * MIB), None);
            assert_eq!(monitor.check_pressure(), None);
            assert_eq!(
                monitor.check_growth(24 * MIB, 48 * MIB),
                Some(crate::scan::LimitReason::MemoryPressure),
                "48 MiB since the sample would leave less than the 64 MiB reserve: {snapshot:?}"
            );
            let selected = monitor.diagnostics.budget_bytes;
            monitor.accept_sample(snapshot);
            assert_eq!(
                monitor.check_growth(24 * MIB, 48 * MIB),
                None,
                "a fresh sample already includes the retained 24 MiB"
            );
            assert_eq!(
                monitor.check_growth(48 * MIB, 72 * MIB),
                Some(crate::scan::LimitReason::MemoryPressure)
            );
            assert_eq!(monitor.diagnostics.budget_bytes, selected);
            monitor.accept_sample(Snapshot::default());
            assert_eq!(
                monitor.check_growth(48 * MIB, 72 * MIB),
                None,
                "unknown headroom must not become zero"
            );
        }
    }
    #[test]
    fn pressure_is_scan_scoped_and_manual_still_stops() {
        let mut monitor =
            Monitor::simulated(BudgetMode::Manual { mib: 4096 }, Snapshot::default(), None);
        monitor.diagnostics.last = Snapshot::default();
        let old = monitor.id;
        pressure(old);
        assert_eq!(
            monitor.reason(1),
            Some(crate::scan::LimitReason::MemoryPressure)
        );
        drop(monitor);
        let mut next = Monitor::simulated(BudgetMode::Automatic, Snapshot::default(), None);
        next.diagnostics.last = Snapshot::default();
        pressure(old);
        assert_eq!(next.reason(1), None);
        next.diagnostics.baseline_process_bytes = Some(100);
        next.diagnostics.last.process = Some(u64::MAX);
        next.diagnostics.last.process_metric = Some(ProcessMetric::Rss);
        next.diagnostics.baseline_process_metric = Some(ProcessMetric::Rss);
        assert_eq!(
            next.reason(1),
            Some(crate::scan::LimitReason::ProcessMemoryLimit)
        );
        next.diagnostics.last.process = None;
        next.diagnostics.last.process_headroom = Some(100 * MIB);
        assert_eq!(
            next.check_growth(10 * MIB, 60 * MIB),
            Some(crate::scan::LimitReason::MemoryPressure)
        );
        next.diagnostics.last.process_headroom = None;
        next.diagnostics.last.process = Some(u64::MAX);
        next.diagnostics.last.process_metric = Some(ProcessMetric::PhysicalFootprint);
        assert_eq!(
            next.reason(1),
            None,
            "different process metrics cannot be compared"
        );
    }
}
