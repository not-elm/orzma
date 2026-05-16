//! Command passing from Tokio worker thread to the CEF UI thread.
//!
//! PoC: uses a simple `Arc<Mutex<VecDeque<CefCommand>>>` queue. The main
//! thread drains the queue between `do_message_loop_work()` calls. Real
//! `cef::post_task(ThreadId::Ui, Some(&mut Task))` integration is deferred
//! to Plan 2 — requires `WrapTask` + `ImplTask` macro pattern with proper
//! cef-rs `RcImpl` ceremony.

use crate::pool::{BrowserPool, CefCommand};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub type CommandQueue = Arc<Mutex<VecDeque<CefCommand>>>;

/// Creates a new empty command queue.
pub fn new_queue() -> CommandQueue {
    Arc::new(Mutex::new(VecDeque::new()))
}

/// Called from any thread to enqueue a command for the CEF UI thread.
pub fn post(queue: &CommandQueue, cmd: CefCommand) {
    queue.lock().expect("queue poisoned").push_back(cmd);
}

/// Called from main thread between `do_message_loop_work()` calls.
pub fn drain(queue: &CommandQueue, pool: &mut BrowserPool) {
    let cmds: Vec<_> = queue.lock().expect("queue poisoned").drain(..).collect();
    for cmd in cmds {
        pool.execute(cmd);
    }
}
