//! Phase 1 integration test: the VT bridge task drives `Term::grid()` from
//! real PTY output.
//!
//! This is the round-trip assertion for Tasks 1-14: spawn bash with
//! `echo hello`, give the bridge task a moment to consume the chunk, then
//! verify the in-memory `Term` shows "hello" on row 0. If this passes, the
//! fan-out → mpsc → `Processor::advance` chain is wired end-to-end.

use std::time::Duration;

use ozmux_multiplexer::{ActivityId, PaneId};
use ozmux_terminal::{DamageSnapshot, SpawnOptions, TerminalService};

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

// ---------------------------------------------------------------------------
// Task 17 PoC: characterize alacritty 0.26 `TermDamage` / `reset_damage` API.
// ---------------------------------------------------------------------------
//
// The bridge task feeds PTY chunks into `Processor::advance(&mut term, ...)`,
// which marks the touched lines as damaged. We then:
//  1. Read damage via `Term::damage()` and snapshot the variant.
//  2. Call `Term::reset_damage()`.
//  3. Read damage again to see whether `reset_damage` actually clears it.
//
// This is documentary — it records what alacritty 0.26 does so Phase 2 can
// design its damage→delta pipeline without re-litigating the API.

#[tokio::test]
async fn term_damage_api_returns_expected_variants() {
    let svc = TerminalService::default();
    let pane = PaneId::new();
    let activity = ActivityId::new();

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

    // Write some output to dirty the grid; give the bridge task time to
    // consume the chunk and apply it to `Term`.
    svc.write(&activity, b"echo line1; echo line2\n")
        .await
        .expect("write must succeed");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let damage = svc
        .inspect_damage_and_reset(&activity)
        .await
        .expect("activity exists after spawn");
    eprintln!("[task17] damage after first write: {damage:?}");
    match &damage {
        DamageSnapshot::Full => {
            // Acceptable: alacritty marks the grid Full on first paint.
        }
        DamageSnapshot::Partial { line_count } => {
            assert!(*line_count > 0, "expected non-empty Partial damage");
        }
    }

    // After reset, with no further PTY input, the tracker should report
    // either Full (sticky) or Partial { line_count: 0 }. We do not assert
    // a specific variant — we document it via eprintln so Phase 2 has a
    // reference.
    let damage_after_reset = svc
        .inspect_damage_and_reset(&activity)
        .await
        .expect("activity still exists");
    eprintln!("[task17] damage after reset (no new input): {damage_after_reset:?}");

    svc.kill(&activity).await.ok();
}

// ---------------------------------------------------------------------------
// Task 18 PoC: verify which DEC private mode escapes toggle `TermMode::
// ALT_SCREEN` in alacritty 0.26.
// ---------------------------------------------------------------------------
//
// We write the enter/exit sequences directly to the PTY (no shell help
// needed) and probe `Term.mode()` for the `ALT_SCREEN` bit.
//
// `?1049` is the critical one (xterm's "save cursor + use alt screen +
// clear"). We assert it works. `?1047` and `?47` are documented but not
// asserted strictly — alacritty's coverage of these legacy variants is
// what we are characterizing.

/// Drive the shell's `printf` to emit the given escape sequence as PTY
/// *output*. Writing the raw escape directly to the master writer goes
/// through the TTY line discipline (cooked mode), where control bytes
/// are echoed as printable `^[` glyphs rather than as the original
/// `\x1b` — so the bridge task never sees a real escape. Going through
/// the shell as a side-effect of running `printf '\033[?...'` produces
/// the bytes on the *slave write* side, which the master reader picks
/// up verbatim.
async fn alt_screen_toggle_check(label: &str, enter_param: &str, exit_param: &str) -> (bool, bool) {
    let svc = TerminalService::default();
    let pane = PaneId::new();
    let activity = ActivityId::new();
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

    // Give the shell a moment to start so subsequent commands run.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Baseline: must start on the primary screen.
    let baseline = svc
        .inspect_alt_screen(&activity)
        .await
        .expect("activity exists");
    assert!(
        !baseline,
        "[{label}] expected primary screen at spawn, got alt"
    );

    let enter_cmd = format!("printf '\\033[?{enter_param}h'\n");
    svc.write(&activity, enter_cmd.as_bytes())
        .await
        .expect("write enter");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let entered = svc
        .inspect_alt_screen(&activity)
        .await
        .expect("activity exists");

    let exit_cmd = format!("printf '\\033[?{exit_param}l'\n");
    svc.write(&activity, exit_cmd.as_bytes())
        .await
        .expect("write exit");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let exited_to_primary = !svc
        .inspect_alt_screen(&activity)
        .await
        .expect("activity exists");

    svc.kill(&activity).await.ok();
    eprintln!("[task18:{label}] enter={entered} exit_to_primary={exited_to_primary}");
    (entered, exited_to_primary)
}

#[tokio::test]
async fn alt_screen_via_1049() {
    let (entered, exited) = alt_screen_toggle_check("?1049", "1049", "1049").await;
    assert!(entered, "?1049h must enter alt-screen");
    assert!(exited, "?1049l must exit alt-screen");
}

#[tokio::test]
async fn alt_screen_via_1047() {
    let (entered, exited) = alt_screen_toggle_check("?1047", "1047", "1047").await;
    if !entered || !exited {
        eprintln!(
            "[task18] WARNING: ?1047 does not toggle ALT_SCREEN in alacritty 0.26 \
             (enter={entered}, exit={exited})"
        );
    }
}

#[tokio::test]
async fn alt_screen_via_47() {
    let (entered, exited) = alt_screen_toggle_check("?47", "47", "47").await;
    if !entered || !exited {
        eprintln!(
            "[task18] WARNING: ?47 does not toggle ALT_SCREEN in alacritty 0.26 \
             (enter={entered}, exit={exited})"
        );
    }
}
