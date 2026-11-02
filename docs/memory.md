# Scan memory policy

The result tree remains in RAM. This policy reduces avoidable memory exhaustion;
it does not make an arbitrary tree fit or guarantee that the OS cannot kill the
process. No database, extra dependency, permission or disk-backed tree is used.

## Settings and admission

Settings default to **Automatic**. **Manual** accepts an integer from 16 through
4096 **MiB** (1 MiB = 1,048,576 bytes), for estimated scanner allocations, not the
entire process. Both modes retain the critical-pressure and process-growth
checks. There is no unlimited mode. The upper input bound limits accidental
entries; it is not a statement that a device can afford 4 GiB.

`settings.json` keeps schema 1 and adds `scan_memory`, either
`{"mode":"automatic"}` or `{"mode":"manual","mib":128}`. Older settings without
the field select Automatic. Invalid numeric budgets fall back to Automatic
without discarding valid language, appearance or window settings. Unknown or
malformed setting types still use the existing corrupt-file recovery.
Desktop/egui and the native Android/iOS settings expose the same preference.
The preference takes effect on the next scan, including a separate-folder scan.

The scanner worker chooses the budget when it actually begins work, after any
older worker has finished. It never increases that budget while scanning.
System measurements do not run in the render loop. All byte arithmetic is
saturating or checked; allocation failure and representation exhaustion leave a
partial result where the relevant collection can still be retained.

Automatic starts with these platform-specific application heuristics:

- Desktop: physical RAM / 16, capped at 2048 MiB; if physical RAM is unknown,
  start with 128 MiB.
- Android/iOS: physical RAM / 32, capped at 128 MiB; if unknown, 32 MiB.
- When system available memory is known, cap desktop by
  `(available - max(512 MiB, physical / 16)) / 4`; mobile by
  `(available - max(256 MiB, physical / 16)) / 6`. Negative remaining space is
  zero. This is not `MemFree`, and it is never offered entirely to the app.
- An available cgroup or Windows job limit also caps the tree at limit / 16.
  A known shared remaining allowance caps it at `(remaining - reserve) / 3`,
  reserving 64 MiB on desktop or 32 MiB on mobile. A group contains other
  processes and, on Linux, charged cache; it is not the process's RSS limit.
- Known process allocation headroom caps the tree at
  `(headroom - reserve) / 3`, with 128 MiB reserved on desktop, 64 MiB on mobile.
  Headroom is calculated with the OS's matching usage metric, never RSS minus a
  virtual/commit limit.
- Android additionally caps Automatic at `getMemoryClass() / 4`. This managed
  heap class is only a conservative sizing recommendation for this mixed
  Java/Rust app, **not** a native allocation limit. No large-heap entitlement is
  requested.

The chosen value is the minimum of applicable caps. A known zero allowance
means that the scan can retain only its root and report a partial result; an
unknown reading never becomes zero. Manual replaces the automatic estimate
cap, not runtime pressure protection.

These fractions and fixed reserves are project heuristics, **not OS-prescribed
safe limits**. Desktop permits a larger absolute allocation and share because
its process is not normally subject to mobile foreground jetsam/LMK policy.
Mobile leaves substantially more room for the native UI, managed heap,
frameworks, providers and foreground/background changes. Division by 3–6
keeps most currently available headroom outside the tree: indexes, search,
sorting, a saved baseline, comparison and exports coexist with it. The maximum
fractions also leave room for the OS and unrelated applications when the
available-memory metric is missing (notably macOS/iOS).

Local synthetic measurements and their limits are in [performance.md](performance.md).
They support allowing hundreds of thousands of nodes while leaving additional
process headroom; they do not calibrate these constants for every mobile device,
allocator, cloud provider or workload. Further device measurements may justify
lower values. The earlier 500,000/100,000 object caps are no longer normal scan
stop conditions. Explicit caller/test object bounds and checked collection
capacity remain. UI work and finite address space still limit useful tree size.

## Platform readings and sources

APIs were reviewed against official documentation on 2026-11-02. A failed call,
missing file, unlimited sentinel or unresolvable controller mount is unknown.
Optional readings are exported as `null`; no permission prompts are added.
`critical: false` means no critical state was observed, not proof that pressure
is absent when its APIs are unavailable.

- **Windows:** `GlobalMemoryStatusEx` supplies total/available physical memory
  and process commit/virtual headroom. `K32GetProcessMemoryInfo` supplies current
  working set (RSS) and private commit. A successfully queried immediate job's
  process commit limit is reduced by **private commit**, and its shared job
  limit can constrain Automatic. Shared current job usage and inaccessible
  ancestor job limits remain unknown; peak counters are not substituted.
  `QueryMemoryResourceNotification` checks the low-memory resource state without
  waiting. These APIs predate the project's Windows CI environment
  (GlobalMemoryStatusEx/notifications: XP; K32 entry point: Windows 7).
  See [GlobalMemoryStatusEx](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-globalmemorystatusex),
  [MEMORYSTATUSEX](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/ns-sysinfoapi-memorystatusex),
  [process counters](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex),
  [job querying and immediate-job semantics](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-queryinformationjobobject),
  [job memory limits](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information),
  [nonblocking pressure state](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-querymemoryresourcenotification).
- **macOS:** `sysctlbyname("hw.memsize")` reads physical RAM;
  `task_info(MACH_TASK_BASIC_INFO)` reads this task's resident size. A
  `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` subscription receives critical events
  (available since macOS 10.9). Free/inactive pages are deliberately not labelled
  system available memory. The public `os_proc_available_memory` API is
  unavailable on macOS; process headroom is unknown here. ABI definitions were
  checked against the installed macOS SDK `mach/task_info.h`, `os/proc.h` and
  `dispatch/source.h`. See Apple's
  [pressure source](https://developer.apple.com/documentation/dispatch/dispatch_source_type_memorypressure),
  [Mach task statistics](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/task_info.h),
  and [memory pressure semantics](https://github.com/apple-oss-distributions/xnu/blob/main/doc/vm/memorystatus_notify.md).
- **Linux:** `/proc/meminfo` distinguishes `MemTotal` from `MemAvailable`
  (kernel estimate, Linux 3.14+); `/proc/self/status` supplies approximate VmRSS
  and VmSize. A finite `Max address space` soft limit in `/proc/self/limits`
  supplies virtual headroom after subtracting VmSize. `/proc/self/cgroup` and
  `/proc/self/mountinfo` resolve v2 or v1 memory-controller membership and mount
  roots, including escaped mount paths. Every visible ancestor up to that mount
  contributes its tightest v2 `memory.max`/`memory.high` or v1 hard limit and
  matching current charged usage. v2 `max` and v1's large unlimited sentinel
  are unknown bounds. Near-exhausted available/group/process headroom is a
  polling pressure signal; historical OOM counters are not replayed as new
  events. Hidden ancestors, missing controllers, inaccessible procfs and PSI
  notifications remain unsupported/unknown, not guessed from host RAM.
  See the kernel [proc documentation](https://www.kernel.org/doc/html/latest/filesystems/proc.html),
  [cgroup v2](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html),
  and [cgroup v1 memory controller](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v1/memory.html).
- **Android (project minSdk 26):** the existing JNI Activity bridge supplies
  `ActivityManager.MemoryInfo.totalMem` (API 16), `availMem`, `threshold`,
  `lowMemory`, and `getMemoryClass()` (API 5). Rust reads this process's VmRSS
  when procfs permits it. Critical legacy trim/low-memory callbacks carry the
  active scan ID and interrupt the matching native query. `UI_HIDDEN` is not a
  pressure event. Android stopped delivering RUNNING_*/COMPLETE and onLowMemory
  callbacks in API 34; the worker therefore also reads MemoryInfo at bounded
  intervals while scanning. This is a deliberate fallback despite Android's
  general preference for notifications over polling, not a promise of advance
  warning on newer Android. No other-process PSS polling or new permission is
  used. See [MemoryInfo](https://developer.android.com/reference/android/app/ActivityManager.MemoryInfo),
  [ActivityManager](https://developer.android.com/reference/android/app/ActivityManager),
  [callback availability](https://developer.android.com/reference/android/content/ComponentCallbacks2).
- **iOS (deployment target 16):** the existing Swift bridge supplies
  `ProcessInfo.physicalMemory`, `task_info(TASK_VM_INFO).phys_footprint`, and
  `os_proc_available_memory()` (iOS 13+). The last is additional dirty-memory
  allowance for this process, not available system RAM or total process limit.
  Its ambiguous zero (non-app or already over limit) stays unknown; memory
  warning notifications remain active. A scan-specific observer captures its
  ID at registration, and is removed at scan completion. System available RAM
  remains unknown. See [physical memory](https://developer.apple.com/documentation/foundation/processinfo/physicalmemory),
  [process allowance](https://developer.apple.com/documentation/os/os_proc_available_memory),
  [memory warnings](https://developer.apple.com/documentation/uikit/uiapplication/didreceivememorywarningnotification),
  and [Apple's guidance on memory termination](https://developer.apple.com/documentation/xcode/reducing-your-app-s-memory-use).

## Runtime accounting and stopping

Before growing node/DFS vectors or the provider's visited-directory set, the
scanner estimates their capacities, owned string/path/error buffers, hash-table
slack, scope metadata, and possible coexistence of old/new vector allocations.
It also rejects a prospective growth that would consume the last sampled
system/group/process allowance minus the runtime reserve, including in Manual.
The guard deducts the entire estimated scanner growth since that sample,
including sequential small allocations. A fresh sample rebases that deduction
at retained allocations, without increasing the chosen budget. Sampled values
can still be stale because of other threads/processes; no allocation is guaranteed.
It reserves fallibly. There is a 64 KiB local workspace allowance and a 2 MiB
provider allowance for bounded pages, decoding and temporary native/bridge
copies. The pending page queue is cleared on close. This is neither a tracing
allocator nor exact accounting of all native objects, allocator fragmentation,
thread stacks, caches or UI allocations. Creating an individual metadata item
can precede its size check. The final diagnostic estimate includes retained
traversal structures at completion; these are freed when the worker returns.

Platform samples occur at start and at most once per 500 ms between entries.
Critical callbacks check a monotonic scan ID, set a flag and invoke the existing
cursor interrupt, without serializing a tree or doing UI work. Old IDs cannot
stop new jobs. A callback/low-memory state or remaining allowance below 64 MiB
desktop / 32 MiB mobile stops with `memory_pressure`, including in Manual.
Measured process growth from a baseline taken before tree allocation stops with
`process_memory_limit` if it exceeds `2 * chosen_budget + reserve`. The metric
(RSS versus iOS physical footprint) is recorded and must match the baseline.
This allowance covers scanner plus other app growth; it never compares absolute
process RSS directly with a tree-only budget. Process samples can be stale or
approximate, and other threads may allocate between checks.

Enumeration stops on a failed growth admission (`memory_limit`), pressure,
process growth, or checked capacity exhaustion (`capacity_limit`). Already
observed children remain; the unfinished listing and its aggregates remain
unknown/partial. User cancellation stays separate. RAII closes desktop cursors;
providers close/finish their native cursor and balance scoped access. Native
providers can still delay interruption; desktop filesystem calls cannot be
forcibly interrupted. Memory policy cannot guarantee cancellation latency for
an unresponsive filesystem or cloud provider.

The UI reports object count, localized reason, chosen budget, estimated scanner
bytes and measured process baseline/latest sample separately. Saved diagnostics
keep their original process metric when opened on another platform. A selected
folder can be scanned separately, without claiming the partial parent was empty.
See [format.md](format.md) for schema 3/CSV 2 and the deliberately independent
saved-file import limits.
