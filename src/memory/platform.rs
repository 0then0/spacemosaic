//! OS APIs run only on the scanner worker (callbacks only set a flag/interrupt).
use super::Snapshot;

#[cfg(any(target_os = "linux", target_os = "android", test))]
fn kib(source: &str, key: &str) -> Option<u64> {
    let line = source.lines().find_map(|l| l.strip_prefix(key))?;
    let mut words = line.split_whitespace();
    let value = words.next()?.parse::<u64>().ok()?;
    (words.next()? == "kB")
        .then(|| value.checked_mul(1024))
        .flatten()
}
#[cfg(any(target_os = "linux", test))]
fn linux_sample() -> Snapshot {
    let info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let mut s = Snapshot {
        physical: kib(&info, "MemTotal:"),
        available: kib(&info, "MemAvailable:"),
        process: kib(&status, "VmRSS:"),
        process_metric: Some(super::ProcessMetric::Rss),
        ..Default::default()
    };
    // RLIMIT_AS is a virtual-address limit. Subtract virtual size, never RSS.
    if let Ok(limits) = std::fs::read_to_string("/proc/self/limits")
        && let Some(limit) = limits
            .lines()
            .find_map(|l| l.strip_prefix("Max address space"))
            .and_then(|l| l.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
    {
        s.process_headroom = kib(&status, "VmSize:").map(|used| limit.saturating_sub(used));
    }
    // Resolve the actual controller mount and membership, including namespaces.
    if let (Ok(groups), Ok(mounts)) = (
        std::fs::read_to_string("/proc/self/cgroup"),
        std::fs::read_to_string("/proc/self/mountinfo"),
    ) {
        apply_cgroups(&mut s, &groups, &mounts, |p| {
            std::fs::read_to_string(p).ok()
        });
    }
    s
}
#[cfg(target_os = "linux")]
pub(super) use linux_sample as sample;
#[cfg(any(target_os = "linux", test))]
fn apply_cgroups(
    s: &mut Snapshot,
    groups: &str,
    mounts: &str,
    read: impl Fn(&std::path::Path) -> Option<String>,
) {
    use std::path::Path;
    fn decode(s: &str) -> String {
        s.replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\")
    }
    fn minimum(target: &mut Option<u64>, n: u64) {
        *target = Some(target.map_or(n, |v| v.min(n)));
    }
    for line in mounts.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<_> = left.split_whitespace().collect();
        let fs: Vec<_> = right.split_whitespace().collect();
        if fields.len() < 5 || fs.len() < 3 {
            continue;
        }
        let v2 = fs[0] == "cgroup2";
        if !v2 && !(fs[0] == "cgroup" && fs[2].split(',').any(|v| v == "memory")) {
            continue;
        }
        for group in groups.lines() {
            let parts: Vec<_> = group.splitn(3, ':').collect();
            if parts.len() != 3
                || if v2 {
                    !parts[1].is_empty()
                } else {
                    !parts[1].split(',').any(|v| v == "memory")
                }
            {
                continue;
            }
            let mount = decode(fields[4]);
            let root = decode(fields[3]);
            let Ok(relative) = Path::new(parts[2]).strip_prefix(&root) else {
                continue;
            };
            // Kernel paths should be normalized. Do not escape the controller mount.
            if relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                continue;
            }
            let mut path = Path::new(&mount).join(relative);
            loop {
                for file in if v2 {
                    ["memory.max", "memory.high"]
                } else {
                    ["memory.limit_in_bytes", "memory.soft_limit_in_bytes"]
                } {
                    let limit = read(&path.join(file)).and_then(|v| v.trim().parse::<u64>().ok());
                    // v1 uses a page-aligned LONG_MAX as its unlimited sentinel.
                    if let Some(limit) = limit.filter(|v| *v < i64::MAX as u64 / 2) {
                        if !v2 && file == "memory.soft_limit_in_bytes" {
                            continue;
                        }
                        minimum(&mut s.group_limit, limit);
                        let usage = read(&path.join(if v2 {
                            "memory.current"
                        } else {
                            "memory.usage_in_bytes"
                        }))
                        .and_then(|v| v.trim().parse::<u64>().ok());
                        if let Some(usage) = usage {
                            minimum(&mut s.group_headroom, limit.saturating_sub(usage));
                        }
                    }
                }
                if path == Path::new(&mount) || !path.pop() {
                    break;
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::Snapshot;
    use std::ffi::{c_char, c_void};
    #[repr(C)]
    #[derive(Default)]
    struct Task {
        sizes: [u64; 3],
        times_and_policy: [i32; 6],
    }
    unsafe extern "C" {
        fn sysctlbyname(
            name: *const c_char,
            old: *mut c_void,
            len: *mut usize,
            new: *mut c_void,
            newlen: usize,
        ) -> i32;
        static mach_task_self_: u32;
        fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
        static _dispatch_source_type_memorypressure: u8;
        fn dispatch_get_global_queue(priority: isize, flags: usize) -> *mut c_void;
        fn dispatch_source_create(
            kind: *const u8,
            handle: usize,
            mask: usize,
            queue: *mut c_void,
        ) -> *mut c_void;
        fn dispatch_set_context(source: *mut c_void, context: *mut c_void);
        fn dispatch_source_set_event_handler_f(
            source: *mut c_void,
            handler: extern "C" fn(*mut c_void),
        );
        fn dispatch_source_set_cancel_handler_f(
            source: *mut c_void,
            handler: extern "C" fn(*mut c_void),
        );
        fn dispatch_resume(source: *mut c_void);
        fn dispatch_source_cancel(source: *mut c_void);
        fn dispatch_release(source: *mut c_void);
    }
    pub fn sample() -> Snapshot {
        let mut physical = 0u64;
        let mut len = 8;
        let mut task = Task::default();
        // SAFETY: ABI matches mach/task_info.h's mach_task_basic_info (48 bytes).
        // The current task port is initialized by the Mach runtime; no foreign task is queried.
        unsafe {
            let ok = sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&mut physical as *mut u64).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            ) == 0
                && len == 8;
            let mut count = (std::mem::size_of::<Task>() / std::mem::size_of::<i32>()) as u32;
            let process = (task_info(
                mach_task_self_,
                20,
                (&mut task as *mut Task).cast(),
                &mut count,
            ) == 0
                && count >= 12)
                .then_some(task.sizes[1]);
            Snapshot {
                physical: ok.then_some(physical),
                process,
                process_metric: process.map(|_| super::super::ProcessMetric::Rss),
                ..Default::default()
            }
        }
    }
    extern "C" fn event(context: *mut c_void) {
        super::super::pressure(unsafe { *(context as *const u64) });
    }
    extern "C" fn dispose(context: *mut c_void) {
        drop(unsafe { Box::from_raw(context as *mut u64) });
    }
    pub fn subscribe(id: u64) -> usize {
        // Listen only for critical events. Context is released after pending handlers finish.
        unsafe {
            let source = dispatch_source_create(
                std::ptr::addr_of!(_dispatch_source_type_memorypressure),
                0,
                4,
                dispatch_get_global_queue(0, 0),
            );
            if source.is_null() {
                return 0;
            }
            dispatch_set_context(source, Box::into_raw(Box::new(id)).cast());
            dispatch_source_set_event_handler_f(source, event);
            dispatch_source_set_cancel_handler_f(source, dispose);
            dispatch_resume(source);
            source as usize
        }
    }
    pub fn unsubscribe(source: usize) {
        if source != 0 {
            unsafe {
                dispatch_source_cancel(source as *mut c_void);
                dispatch_release(source as *mut c_void);
            }
        }
    }
}
#[cfg(target_os = "macos")]
pub(super) use mac::sample;

#[cfg(any(target_os = "windows", test))]
mod windows {
    use super::Snapshot;
    use std::ffi::c_void;
    #[repr(C)]
    #[derive(Default)]
    struct Status {
        length: u32,
        load: u32,
        values: [u64; 7],
    }
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        faults: u32,
        values: [usize; 9],
    }
    #[repr(C)]
    #[derive(Default)]
    struct Basic {
        time: [i64; 2],
        flags: u32,
        min: usize,
        max: usize,
        active: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Job {
        basic: Basic,
        io: [u64; 6],
        process_limit: usize,
        job_limit: usize,
        peak_process: usize,
        peak_job: usize,
    }
    #[cfg_attr(target_os = "windows", link(name = "kernel32"))]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(s: *mut Status) -> i32;
        fn GetCurrentProcess() -> *mut c_void;
        fn CreateMemoryResourceNotification(kind: i32) -> *mut c_void;
        fn QueryMemoryResourceNotification(handle: *mut c_void, state: *mut i32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn K32GetProcessMemoryInfo(p: *mut c_void, c: *mut Counters, size: u32) -> i32;
        fn QueryInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *mut c_void,
            size: u32,
            returned: *mut u32,
        ) -> i32;
    }
    pub fn sample() -> Snapshot {
        let mut status = Status {
            length: std::mem::size_of::<Status>() as u32,
            ..Default::default()
        };
        let mut counters = Counters {
            cb: std::mem::size_of::<Counters>() as u32,
            ..Default::default()
        };
        let mut job = Job::default();
        unsafe {
            let mut s = Snapshot::default();
            if GlobalMemoryStatusEx(&mut status) != 0 {
                s.physical = Some(status.values[0]);
                s.available = Some(status.values[1]);
                // ullAvailPageFile is commit headroom available to THIS process.
                s.process_headroom = Some(status.values[3].min(status.values[5]));
            }
            let has_process = K32GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters,
                std::mem::size_of::<Counters>() as u32,
            ) != 0;
            if has_process {
                s.process = Some(counters.values[1] as u64);
                s.process_metric = Some(super::super::ProcessMetric::Rss);
            }
            if QueryInformationJobObject(
                std::ptr::null_mut(),
                9,
                (&mut job as *mut Job).cast(),
                std::mem::size_of::<Job>() as u32,
                std::ptr::null_mut(),
            ) != 0
            {
                if has_process && job.basic.flags & 0x100 != 0 {
                    let remaining = job.process_limit.saturating_sub(counters.values[8]) as u64;
                    s.process_headroom =
                        Some(s.process_headroom.map_or(remaining, |v| v.min(remaining)));
                }
                if job.basic.flags & 0x200 != 0 {
                    s.group_limit = Some(job.job_limit as u64);
                }
                // Shared job usage/nested limits may be unavailable. Never infer
                // remaining allowance from a peak counter.
            }
            let notification = CreateMemoryResourceNotification(0);
            if !notification.is_null() {
                let mut low = 0;
                s.critical =
                    QueryMemoryResourceNotification(notification, &mut low) != 0 && low != 0;
                CloseHandle(notification);
            }
            s
        }
    }
    #[cfg(test)]
    #[test]
    fn native_64_bit_abi_layouts() {
        if cfg!(target_pointer_width = "64") {
            assert_eq!(std::mem::size_of::<Status>(), 64);
            assert_eq!(std::mem::size_of::<Counters>(), 80);
            assert_eq!(std::mem::size_of::<Basic>(), 64);
            assert_eq!(std::mem::size_of::<Job>(), 144);
        }
        let _: fn() -> Snapshot = sample; // Type-check without calling Windows APIs on the host.
    }
}
#[cfg(target_os = "windows")]
pub(super) use windows::sample;

#[cfg(all(any(target_os = "android", target_os = "ios"), feature = "gui"))]
fn mobile_request(value: serde_json::Value) -> Option<String> {
    #[cfg(target_os = "android")]
    let response = (crate::android::storage_transport())(&value.to_string());
    #[cfg(target_os = "ios")]
    let response = crate::ios::request(&value.to_string());
    response.ok()
}
#[cfg(all(any(target_os = "android", target_os = "ios"), feature = "gui"))]
pub(super) fn sample() -> Snapshot {
    let mut snapshot = mobile_request(serde_json::json!({"op":"memory"}))
        .and_then(|s| serde_json::from_str::<Snapshot>(&s).ok())
        .unwrap_or_default();
    #[cfg(target_os = "android")]
    {
        snapshot.process = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| kib(&s, "VmRSS:"));
        snapshot.process_metric = snapshot.process.map(|_| super::ProcessMetric::Rss);
    }
    #[cfg(target_os = "ios")]
    {
        // Swift reports physical footprint, not RSS.
        snapshot = snapshot.sanitized();
    }
    snapshot
}
#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "windows",
    all(any(target_os = "android", target_os = "ios"), feature = "gui")
)))]
pub(super) fn sample() -> Snapshot {
    Snapshot::default()
}

pub(super) type Subscription = usize;
pub(super) fn subscribe(id: u64) -> Subscription {
    #[cfg(target_os = "macos")]
    {
        mac::subscribe(id)
    }
    #[cfg(all(any(target_os = "android", target_os = "ios"), feature = "gui"))]
    {
        let _ = mobile_request(serde_json::json!({"op":"memory_start","id":id}));
        0
    }
    #[cfg(not(any(
        target_os = "macos",
        all(any(target_os = "android", target_os = "ios"), feature = "gui")
    )))]
    {
        let _ = id;
        0
    }
}
pub(super) fn unsubscribe(id: u64, subscription: &mut Subscription) {
    #[cfg(target_os = "macos")]
    mac::unsubscribe(*subscription);
    #[cfg(all(any(target_os = "android", target_os = "ios"), feature = "gui"))]
    {
        let _ = mobile_request(serde_json::json!({"op":"memory_stop","id":id}));
    }
    let _ = (id, subscription);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_unknowns_overflow_and_nested_cgroups() {
        let _: fn() -> Snapshot = linux_sample;
        assert_eq!(kib("MemAvailable: 42 kB", "MemAvailable:"), Some(43008));
        for s in [
            "MemAvailable: -1 kB",
            "MemAvailable: 18446744073709551615 kB",
            "MemAvailable: 42 MB",
            "",
        ] {
            assert_eq!(kib(s, "MemAvailable:"), None);
        }
        let mut s = Snapshot::default();
        apply_cgroups(
            &mut s,
            "0::/a/b",
            "29 23 0:26 / /sys/fs/cgroup rw - cgroup2 cgroup rw",
            |p| match p.to_str().unwrap() {
                "/sys/fs/cgroup/a/b/memory.max" => Some("max".into()),
                "/sys/fs/cgroup/a/memory.max" => Some("1000".into()),
                "/sys/fs/cgroup/a/memory.current" => Some("900".into()),
                "/sys/fs/cgroup/memory.max" => Some("5000".into()),
                "/sys/fs/cgroup/memory.current" => Some("3000".into()),
                _ => None,
            },
        );
        assert_eq!(s.group_limit, Some(1000));
        assert_eq!(s.group_headroom, Some(100));
        let mut s = Snapshot::default();
        apply_cgroups(
            &mut s,
            "5:memory:/slice/task",
            "29 23 0:26 /slice /cg rw - cgroup cgroup rw,memory",
            |p| match p.to_str().unwrap() {
                "/cg/task/memory.limit_in_bytes" => Some("1024".into()),
                "/cg/task/memory.usage_in_bytes" => Some("2048".into()),
                _ => None,
            },
        );
        assert_eq!(s.group_headroom, Some(0));
    }
}
