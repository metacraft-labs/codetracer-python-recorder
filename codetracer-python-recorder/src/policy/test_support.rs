//! Shared test-only utilities for the `policy` submodule tree.
//!
//! The three test modules (`policy::tests`, `policy::env::tests`,
//! `policy::ffi::tests`) all mutate the global env block via
//! `std::env::set_var`.  Without a shared mutex cargo test's default
//! parallel execution lets two tests in *different* submodules race
//! on the same env keys — test A's "proxies,fd" overwrites test B's
//! "invalid-token" mid-`configure_policy_from_env`.
//!
//! `ENV_MUTEX` is a single process-wide lock that all env-touching
//! tests acquire through `EnvGuard::new()`.  The guard also restores
//! the env block on Drop so a panicking test doesn't poison its
//! neighbours.
//!
//! On Windows the symptom is especially loud because env-var
//! visibility flips per-thread without warning.

use std::sync::{Mutex, MutexGuard};

pub(crate) static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// Guard that holds the shared `ENV_MUTEX` for the test's duration
/// and clears the recorder-policy env keys on Drop so a failing
/// assertion doesn't leak state into the next test.
pub(crate) struct EnvGuard {
    _guard: Option<MutexGuard<'static, ()>>,
    cleanup_keys: &'static [&'static str],
}

impl EnvGuard {
    pub(crate) fn new(cleanup_keys: &'static [&'static str]) -> Self {
        // Recover from a poisoned mutex: a prior test panicked while
        // holding the lock.  We still want to take the lock so the
        // current test runs serially — discarding the poison is safe
        // because env vars are removed in our Drop anyway.
        let guard = ENV_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        Self {
            _guard: Some(guard),
            cleanup_keys,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for key in self.cleanup_keys {
            std::env::remove_var(key);
        }
        // _guard drops automatically afterwards, releasing the mutex.
    }
}
