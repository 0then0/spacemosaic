//! Debounced search with one lazy worker and at most one replacement request.
use super::*;
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub(super) const DEBOUNCE: Duration = Duration::from_millis(200);
pub(super) const BACKGROUND_THRESHOLD: usize = 50_000;

#[derive(Default)]
pub(super) struct SearchState {
    pub query: String,
    deadline: Option<Instant>,
    worker: Option<Worker>,
    job: Option<Job>,
}
impl SearchState {
    pub fn edited(&mut self, now: Instant) {
        self.cancel();
        self.deadline = (!self.query.is_empty()).then_some(now + DEBOUNCE);
    }
    pub fn clear(&mut self) {
        self.query.clear();
        self.deadline = None;
        self.cancel();
    }
    pub fn cancel(&mut self) {
        self.job = None;
    }
    pub fn pending(&self) -> bool {
        self.deadline.is_some() || self.job.is_some()
    }
    pub fn remaining(&mut self, now: Instant) -> Option<Duration> {
        let deadline = self.deadline?;
        if now >= deadline {
            self.deadline = None;
            None
        } else {
            Some(deadline - now)
        }
    }
    pub fn start(&mut self, request: Request) {
        self.job = Some(
            self.worker
                .get_or_insert_with(Worker::default)
                .submit(request),
        );
    }
    pub fn take(&mut self) -> Option<Vec<usize>> {
        let rows = self.job.as_ref()?.receiver.try_recv().ok()?;
        self.job = None;
        Some(rows)
    }
    pub fn repaint(&self, context: &egui::Context) {
        if let Some(deadline) = self.deadline {
            context.request_repaint_after(deadline.saturating_duration_since(Instant::now()));
        } else if self.job.is_some() {
            context.request_repaint_after(Duration::from_millis(100));
        }
    }
}

pub(super) struct Request {
    pub result: Arc<ScanResult>,
    pub query: String,
    pub group: Option<Vec<usize>>,
    pub files_only: bool,
    pub sort: Sort,
    pub allocated: bool,
}
impl Request {
    pub fn run(&self, cancel: &AtomicBool) -> Option<Vec<usize>> {
        let query = self.query.to_lowercase();
        let count = self
            .group
            .as_ref()
            .map_or(self.result.nodes.len(), Vec::len);
        let mut rows = Vec::new();
        for position in 0..count {
            if position % 1024 == 0 && cancel.load(Ordering::Relaxed) {
                return None;
            }
            let index = self
                .group
                .as_ref()
                .map_or(position, |group| group[position]);
            let node = &self.result.nodes[index];
            if self.group.is_none() && (index == 0 || (self.files_only && node.kind != Kind::File))
            {
                continue;
            }
            if query.is_empty()
                || node.name.to_lowercase().contains(&query)
                || node.locator.to_lowercase().contains(&query)
            {
                rows.push(index);
            }
        }
        self.sort_rows(&mut rows, cancel)?;
        (!cancel.load(Ordering::Relaxed)).then_some(rows)
    }
    fn compare(&self, a: usize, b: usize) -> std::cmp::Ordering {
        let nodes = &self.result.nodes;
        if self.sort == Sort::Size && self.allocated {
            return nodes[b]
                .allocated
                .known
                .cmp(&nodes[a].allocated.known)
                .then(a.cmp(&b));
        }
        let name = || scan::compare_names(&nodes[a].name, &nodes[b].name).then(a.cmp(&b));
        if self.sort == Sort::Size {
            nodes[b]
                .logical
                .known
                .cmp(&nodes[a].logical.known)
                .then_with(name)
        } else {
            name()
        }
    }
    /// Sort bounded runs, then merge with cancellation checkpoints. A cancelled
    /// wide match never holds up the latest query for a full uninterruptible sort.
    fn sort_rows(&self, rows: &mut [usize], cancel: &AtomicBool) -> Option<()> {
        const RUN: usize = 2048;
        for chunk in rows.chunks_mut(RUN) {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            chunk.sort_unstable_by(|a, b| self.compare(*a, *b));
        }
        if rows.len() <= RUN {
            return Some(());
        }
        let mut scratch = vec![0; rows.len()];
        let mut width = RUN;
        while width < rows.len() {
            for start in (0..rows.len()).step_by(width * 2) {
                let middle = (start + width).min(rows.len());
                let end = (middle + width).min(rows.len());
                let (mut left, mut right) = (start, middle);
                for (offset, target) in scratch[start..end].iter_mut().enumerate() {
                    if offset % 1024 == 0 && cancel.load(Ordering::Relaxed) {
                        return None;
                    }
                    if left < middle
                        && (right == end || self.compare(rows[left], rows[right]).is_le())
                    {
                        *target = rows[left];
                        left += 1;
                    } else {
                        *target = rows[right];
                        right += 1;
                    }
                }
            }
            rows.copy_from_slice(&scratch);
            width *= 2;
        }
        Some(())
    }
}

struct Job {
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Vec<usize>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
struct Task {
    request: Request,
    cancel: Arc<AtomicBool>,
    sender: mpsc::SyncSender<Vec<usize>>,
}
#[derive(Default)]
struct Queue {
    pending: Option<Task>,
    active: Option<Arc<AtomicBool>>,
    stopped: bool,
}
struct Worker {
    queue: Arc<(Mutex<Queue>, Condvar)>,
}
impl Default for Worker {
    fn default() -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let background = queue.clone();
        std::thread::Builder::new()
            .name("result-search".into())
            .spawn(move || {
                loop {
                    let task = {
                        let (lock, wake) = &*background;
                        let mut queue = lock.lock().unwrap_or_else(|p| p.into_inner());
                        while queue.pending.is_none() && !queue.stopped {
                            queue = wake.wait(queue).unwrap_or_else(|p| p.into_inner());
                        }
                        if queue.stopped {
                            break;
                        }
                        let task = queue.pending.take().expect("pending search");
                        queue.active = Some(task.cancel.clone());
                        task
                    };
                    if let Some(rows) = task.request.run(&task.cancel) {
                        let _ = task.sender.try_send(rows);
                        bridge::wake();
                    }
                    background
                        .0
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .active = None;
                }
            })
            .expect("create search worker");
        Self { queue }
    }
}
impl Worker {
    fn submit(&self, request: Request) -> Job {
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut queue = self.queue.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(active) = &queue.active {
            active.store(true, Ordering::Relaxed);
        }
        queue.pending = Some(Task {
            request,
            cancel: cancel.clone(),
            sender,
        });
        self.queue.1.notify_one();
        Job { cancel, receiver }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let mut queue = self.queue.0.lock().unwrap_or_else(|p| p.into_inner());
        queue.stopped = true;
        queue.pending = None;
        if let Some(active) = &queue.active {
            active.store(true, Ordering::Relaxed);
        }
        self.queue.1.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_discards_an_already_delivered_response() {
        let mut search = SearchState::default();
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        search.job = Some(Job {
            cancel: cancel.clone(),
            receiver,
        });
        sender.send(vec![999]).unwrap();
        search.query = "new query".into();
        search.edited(Instant::now());
        assert!(cancel.load(Ordering::Relaxed));
        assert!(search.take().is_none());
    }

    #[test]
    fn typing_restarts_the_deadline_and_clearing_is_immediate() {
        let mut search = SearchState::default();
        let now = Instant::now();
        search.query = "a".into();
        search.edited(now);
        assert_eq!(search.remaining(now), Some(DEBOUNCE));
        search.query.push('b');
        search.edited(now + Duration::from_millis(150));
        assert_eq!(
            search.remaining(now + DEBOUNCE),
            Some(Duration::from_millis(150))
        );
        assert_eq!(search.remaining(now + Duration::from_millis(350)), None);
        search.query.clear();
        search.edited(now);
        assert_eq!(search.remaining(now), None);
    }
}
