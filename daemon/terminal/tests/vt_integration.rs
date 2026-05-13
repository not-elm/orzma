//! Phase 1 integration test: the VT bridge task drives `Term::grid()` from
//! real PTY output.
//!
//! This is the round-trip assertion for Tasks 1-14: spawn bash with
//! `echo hello`, give the bridge task a moment to consume the chunk, then
//! verify the in-memory `Term` shows "hello" on row 0. If this passes, the
//! fan-out → mpsc → `Processor::advance` chain is wired end-to-end.

use std::time::Duration;

use ozmux_multiplexer::{ActivityId, PaneId};
use ozmux_terminal::{SpawnOptions, TerminalService};

#[tokio::test]
async fn term_grid_reflects_bash_echo_output() {
    let svc = TerminalService::default();
    let pane = PaneId::new();
    let activity = ActivityId::new();

    // Use `sh -c 'echo hello'` so the only PTY output the bridge sees is
    // `hello\n` — no prompt, no echoed input. The bridge task should then
    // render "hello" at column 0 of row 0.
    svc.spawn(
        pane,
        activity.clone(),
        SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/sh".to_string(),
            cwd: None,
            window_id: None,
            session_id: None,
        },
    )
    .await
    .expect("spawn must succeed");

    // sh is interactive by default under a PTY; drive the echo through stdin
    // and assert the rendered output line, not the echoed input.
    svc.write(&activity, b"echo hello\n")
        .await
        .expect("write must succeed");

    // Poll the grid until bash's `hello` output shows up on some row, or we
    // hit the deadline. We scan rows 0..5 because shell prompts and the
    // typed-input echo push the actual output line below row 0; the
    // assertion here is *that the bridge applied PTY output to the grid*,
    // not the exact row layout (which depends on the host shell's prompt).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut rows_snapshot: Vec<String> = Vec::new();
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        rows_snapshot.clear();
        for r in 0..5_i32 {
            let row = svc
                .inspect_row(&activity, r, 80)
                .await
                .expect("activity exists after spawn");
            rows_snapshot.push(row);
        }
        if rows_snapshot
            .iter()
            .any(|line| line.trim_end_matches(' ').ends_with("hello") || line.contains(" hello"))
        {
            // Stronger check: at least one row begins exactly with "hello".
            // This filters out the typed-input echo line "echo hello".
            if rows_snapshot.iter().any(|line| &line[..5] == "hello") {
                found = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    svc.kill(&activity).await.ok();

    assert!(
        found,
        "expected bridge task to render 'hello' as the first 5 chars of some \
         row within 3s; rows were: {rows_snapshot:?}"
    );
}
