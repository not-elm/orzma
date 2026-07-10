//! Test-only support shared across `src/`'s unit test modules. Compiled
//! only under `#[cfg(test)]` (see the `mod test_support;` declaration in
//! `main.rs`).

use std::ffi::{OsStr, OsString};

/// RAII guard for a process-environment variable. `EnvVarGuard::set` /
/// `::unset` snapshot the prior value and restore it (or remove the var, if
/// it was previously unset) on drop, even on panic — so a test that panics
/// mid-scope does not leak a stale-set or stale-unset env var into the next
/// test.
///
/// The caller MUST hold `crate::configs::env_guard()` for the full lifetime
/// of every `EnvVarGuard` to keep env mutations serialized across tests.
pub(crate) struct EnvVarGuard {
    key: &'static str,
    prior: Option<OsString>,
}

impl EnvVarGuard {
    /// Sets `key` to `value`, snapshotting the prior value for restoration on drop.
    pub(crate) fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let prior = std::env::var_os(key);
        // SAFETY: caller holds env_guard() for the duration of this guard.
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, prior }
    }

    /// Removes `key`, snapshotting the prior value for restoration on drop.
    pub(crate) fn unset(key: &'static str) -> Self {
        let prior = std::env::var_os(key);
        // SAFETY: caller holds env_guard() for the duration of this guard.
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, prior }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: caller still holds env_guard() (LIFO drop order keeps
        // this ahead of the MutexGuard's own drop within each test scope).
        unsafe {
            match self.prior.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
