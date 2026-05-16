//! Command passing from the Tokio worker thread to the CEF UI thread.
//!
//! Phase A Task A2: replaces the Plan 1 `Arc<Mutex<VecDeque>>` + main-thread
//! drain loop with `cef::post_task(ThreadId::UI, ExecuteTask)`. Every
//! `CefCommand` is wrapped in an `ExecuteTask` whose `execute()` acquires the
//! `BrowserPool` mutex (uncontended in practice: only the CEF UI thread locks
//! it) and dispatches.

use crate::pool::{BrowserPool, CefCommand};
use cef::rc::Rc as _;
use cef::{ImplTask, Task, ThreadId, WrapTask, post_task, wrap_task};
use std::sync::{Arc, Mutex};

/// Shared handle to the `BrowserPool` that can be cloned and sent across threads.
///
/// Wraps `Arc<Mutex<BrowserPool>>` so the Tokio worker and the CEF UI thread
/// can both reach the pool without exposing the raw `Arc` at call sites.
#[derive(Clone)]
pub struct PoolHandle {
    pool: Arc<Mutex<BrowserPool>>,
}

impl PoolHandle {
    /// Creates a new `PoolHandle` that takes ownership of `pool`.
    pub fn new(pool: BrowserPool) -> Self {
        Self {
            pool: Arc::new(Mutex::new(pool)),
        }
    }

    /// Reads `shutdown_requested` under a brief lock without holding it across
    /// the caller's loop body.
    pub fn snapshot_shutdown_requested(&self) -> bool {
        self.pool.lock().expect("pool poisoned").shutdown_requested
    }

    /// Forces `shutdown_requested` without going through `post_task`.
    ///
    /// Only intended as a fallback when `post_task` fails during shutdown
    /// (i.e., CEF is already tearing down). In that case the process will exit
    /// momentarily regardless; this just breaks the message loop promptly.
    pub fn force_shutdown(&self) {
        self.pool.lock().expect("pool poisoned").shutdown_requested = true;
    }

    /// Test-only helper: runs a closure with mutable access to the inner pool.
    ///
    /// # Note
    /// `#[doc(hidden)]` rather than `#[cfg(test)]` so that integration test
    /// crates (which compile the lib without `cfg(test)`) can still reach it.
    /// Named `_for_tests` to make its purpose self-documenting at call sites.
    /// `cef::post_task` requires a live `CefInitialize` which unit tests cannot
    /// provide, so tests dispatch commands through this accessor instead.
    #[doc(hidden)]
    pub fn with_pool_mut_for_tests<F: FnOnce(&mut BrowserPool)>(&self, f: F) {
        f(&mut self.pool.lock().expect("pool poisoned"));
    }
}

/// Error returned when `cef::post_task` refuses the task.
#[derive(thiserror::Error, Debug)]
pub enum PostError {
    /// `cef::post_task` returned 0, which means CEF is shutting down or this
    /// call was made from the wrong thread before `CefInitialize`.
    #[error("cef::post_task returned 0 (CEF shutting down or called before CefInitialize)")]
    PostFailed,
}

// NOTE: both fields are `Arc<Mutex<…>>` because `wrap_task!` auto-derives
// `Clone` on `ExecuteTask` (cef-rs refcounting model) and bare `Mutex<T>` is
// not `Clone`. The `Arc` here is for the macro's needs, not multi-owner
// semantics; do not "simplify" it to `Mutex<Option<…>>`.
wrap_task! {
    struct ExecuteTask {
        pool: Arc<Mutex<BrowserPool>>,
        cmd: Arc<Mutex<Option<CefCommand>>>,
    }

    impl Task {
        fn execute(&self) {
            // NOTE: take() ensures idempotence — if CEF ever re-runs this task
            // the second call is a no-op rather than a double-execute.
            if let Some(cmd) = self.cmd.lock().expect("cmd poisoned").take() {
                self.pool.lock().expect("pool poisoned").execute(cmd);
            }
        }
    }
}

/// Posts `cmd` to the CEF UI thread via `cef::post_task`.
///
/// Returns `Err(PostError::PostFailed)` when `cef::post_task` returns 0, which
/// signals that CEF is shutting down or `CefInitialize` has not been called yet.
pub fn post(handle: &PoolHandle, cmd: CefCommand) -> Result<(), PostError> {
    let cmd_slot = Arc::new(Mutex::new(Some(cmd)));
    let mut task = ExecuteTask::new(Arc::clone(&handle.pool), cmd_slot);
    let posted = post_task(ThreadId::UI, Some(&mut task));
    if posted == 1 {
        Ok(())
    } else {
        Err(PostError::PostFailed)
    }
}
