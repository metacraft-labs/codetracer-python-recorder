//! Thread-local line snapshot store used to attribute IO to the most recent step.

use runtime_tracing::{Line, PathId};
use std::collections::HashMap;
use std::sync::RwLock;
use std::thread::ThreadId;
use std::time::Instant;

/// Identifier that helps correlate IO chunks with the owning Python frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameId(u64);

impl FrameId {
    /// Build a frame id from a raw pointer cast or any unique integer.
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the raw identifier value.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn as_raw(self) -> u64 {
        self.0
    }
}

/// Snapshot for the most recent executed line per Python thread.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug)]
pub struct LineSnapshot {
    path_id: PathId,
    line: Line,
    frame_id: FrameId,
    captured_at: Instant,
}

#[cfg_attr(not(test), allow(dead_code))]
impl LineSnapshot {
    pub fn new(path_id: PathId, line: Line, frame_id: FrameId) -> Self {
        Self {
            path_id,
            line,
            frame_id,
            captured_at: Instant::now(),
        }
    }

    pub fn path_id(&self) -> PathId {
        self.path_id
    }

    pub fn line(&self) -> Line {
        self.line
    }

    pub fn frame_id(&self) -> FrameId {
        self.frame_id
    }

    pub fn captured_at(&self) -> Instant {
        self.captured_at
    }
}

/// Concurrent store recording the latest line snapshot per Python thread.
#[derive(Default)]
pub struct LineSnapshotStore {
    inner: RwLock<HashMap<ThreadId, LineSnapshot>>,
}

impl LineSnapshotStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    /// Record or update the snapshot for a thread.
    pub fn record(&self, thread_id: ThreadId, path_id: PathId, line: Line, frame_id: FrameId) {
        let snapshot = LineSnapshot::new(path_id, line, frame_id);
        let mut guard = self.inner.write().expect("lock poisoned");
        guard.insert(thread_id, snapshot);
    }

    /// Fetch the latest snapshot for a thread if present.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn snapshot_for_thread(&self, thread_id: ThreadId) -> Option<LineSnapshot> {
        let guard = self.inner.read().expect("lock poisoned");
        guard.get(&thread_id).cloned()
    }

    /// Remove a snapshot when a thread terminates.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn remove(&self, thread_id: ThreadId) -> Option<LineSnapshot> {
        let mut guard = self.inner.write().expect("lock poisoned");
        guard.remove(&thread_id)
    }

    /// Clear all stored snapshots.
    pub fn clear(&self) {
        let mut guard = self.inner.write().expect("lock poisoned");
        guard.clear();
    }

    /// Count tracked threads, used in tests.
    #[cfg(test)]
    pub fn tracked_threads(&self) -> usize {
        let guard = self.inner.read().expect("lock poisoned");
        guard.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_tracing::Line;
    use std::sync::Arc;
    use std::thread;

    fn make_snapshot(store: &LineSnapshotStore, line: i64) -> LineSnapshot {
        let tid = thread::current().id();
        store.record(tid, PathId(7), Line(line), FrameId::from_raw(line as u64));
        store
            .snapshot_for_thread(tid)
            .expect("snapshot should be recorded")
    }

    #[test]
    fn records_and_reads_snapshot() {
        let store = LineSnapshotStore::new();
        let snapshot = make_snapshot(&store, 10);
        assert_eq!(snapshot.path_id(), PathId(7));
        assert_eq!(snapshot.line(), Line(10));
        assert_eq!(snapshot.frame_id().as_raw(), 10);
    }

    #[test]
    fn removes_snapshot_for_thread() {
        let store = LineSnapshotStore::new();
        let tid = thread::current().id();
        store.record(tid, PathId(3), Line(20), FrameId::from_raw(1));
        assert!(store.snapshot_for_thread(tid).is_some());
        assert!(store.remove(tid).is_some());
        assert!(store.snapshot_for_thread(tid).is_none());
    }

    #[test]
    fn concurrent_updates_do_not_panic() {
        let store = Arc::new(LineSnapshotStore::new());
        let threads: Vec<_> = (0..8)
            .map(|idx| {
                let store_ref = Arc::clone(&store);
                thread::spawn(move || {
                    for step in 0..32usize {
                        let line = Line(step as i64);
                        let frame_raw = (idx as u64) * 100 + step as u64;
                        store_ref.record(
                            thread::current().id(),
                            PathId(idx),
                            line,
                            FrameId::from_raw(frame_raw),
                        );
                    }
                })
            })
            .collect();

        for handle in threads {
            handle.join().expect("thread should finish");
        }

        assert!(store.tracked_threads() <= 8);
    }
}
