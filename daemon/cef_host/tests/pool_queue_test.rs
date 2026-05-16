//! Verifies the BrowserPool + queue drain pattern works in isolation
//! (no CEF runtime needed).

use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_cef_host::pool::{BrowserPool, CefCommand};
use ozmux_cef_host::post_command::{drain, new_queue, post};

#[test]
fn shutdown_command_sets_pool_flag() {
    let queue = new_queue();
    let mut pool = BrowserPool::new();
    post(&queue, CefCommand::Shutdown);
    drain(&queue, &mut pool);
    assert!(pool.shutdown_requested);
}

#[test]
fn browser_create_stub_inserts_entry() {
    let queue = new_queue();
    let mut pool = BrowserPool::new();
    assert_eq!(pool.browser_count(), 0);

    post(
        &queue,
        CefCommand::BrowserCreate {
            aid: ActivityId("a1".into()),
            initial_url: "https://example.com/".into(),
            epoch: 1,
            shm_fd: -1,
        },
    );
    drain(&queue, &mut pool);
    assert_eq!(pool.browser_count(), 1);

    post(
        &queue,
        CefCommand::Close {
            aid: ActivityId("a1".into()),
        },
    );
    drain(&queue, &mut pool);
    assert_eq!(pool.browser_count(), 0);
}

#[test]
fn drain_processes_all_pending_commands() {
    let queue = new_queue();
    let mut pool = BrowserPool::new();
    for i in 0..5 {
        post(
            &queue,
            CefCommand::BrowserCreate {
                aid: ActivityId(format!("a{i}")),
                initial_url: "about:blank".into(),
                epoch: 1,
                shm_fd: -1,
            },
        );
    }
    drain(&queue, &mut pool);
    assert_eq!(pool.browser_count(), 5);
}
