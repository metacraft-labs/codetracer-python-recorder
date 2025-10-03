//! Store the latest execution snapshot per OS thread.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::thread::{self, ThreadId};

use log::warn;
use runtime_tracing::{Line, PathId};

/// Snapshot of the last recorded step for a thread.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct SnapshotEntry {
    pub path_id: PathId,
    pub line: Line,
    pub frame_id: usize,
}

#[derive(Default, Clone, Debug)]
pub struct ThreadSnapshotStore {
    inner: std::sync::Arc<Mutex<HashMap<ThreadId, SnapshotEntry>>>,
    last_global: std::sync::Arc<Mutex<Option<SnapshotEntry>>>,
}

impl ThreadSnapshotStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a snapshot for the current thread.
    pub fn update_current(&self, entry: SnapshotEntry) {
        let thread_id = thread::current().id();
        let mut store = lock(&self.inner);
        store.insert(thread_id, entry.clone());
        drop(store);

        let mut global = lock(&self.last_global);
        *global = Some(entry);
    }

    /// Remove any stored snapshot for the current thread.
    pub fn clear_current(&self) {
        let thread_id = thread::current().id();
        let mut store = lock(&self.inner);
        store.remove(&thread_id);
    }

    /// Return the latest snapshot for a given thread id.
    #[allow(dead_code)]
    pub fn snapshot_for(&self, thread_id: ThreadId) -> Option<SnapshotEntry> {
        let store = lock(&self.inner);
        store.get(&thread_id).cloned()
    }

    /// Return the most recent snapshot observed across all threads.
    #[allow(dead_code)]
    pub fn latest(&self) -> Option<SnapshotEntry> {
        let global = lock(&self.last_global);
        global.clone()
    }

    /// Clear all stored snapshots.
    pub fn reset(&self) {
        let mut store = lock(&self.inner);
        store.clear();
        drop(store);
        let mut global = lock(&self.last_global);
        *global = None;
    }
}

fn lock<T>(mutex: &std::sync::Arc<Mutex<T>>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            warn!("ThreadSnapshotStore mutex poisoned; continuing with recovered state");
            poisoned.into_inner()
        }
    }
}
