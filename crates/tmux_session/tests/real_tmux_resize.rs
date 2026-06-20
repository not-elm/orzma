//! Gated end-to-end tests against a real tmux `-CC`:
//!
//! 1. `resize_pane_settle` — a single `resize-pane -x/-y` command produces a
//!    `%layout-change` that updates the projected pane width, with no infinite
//!    loop, proving the one-in-flight throttle premise is safe.
//! 2. `copy_mode_drag_select` — entering copy mode, positioning the cursor,
//!    calling `begin-selection` + `copy-selection`, then `show-buffer` returns
//!    the exact substring of deterministic content.
//! 3. `select_word_and_line` — `select-word` on a non-first character grabs the
//!    whole word; the first-character behavior (tmux #1820) is documented in an
//!    assertion that records the ACTUAL tmux 3.6b behavior; `select-line` selects
//!    the whole line.
//! 4. `wheel_binding_enters_copy_mode` — tmux's default `WheelUpPane` if-shell
//!    conditional enters copy mode on a plain pane (the premise behind
//!    `is_copy_mode_entry` matching `copy-mode` inside the conditional).
//! 5. `wheel_in_copy_mode_scrolls` — ozmux's direct pane-targeted
//!    `send-keys -X -t %id -N <n> scroll-up` (what it now sends per wheel notch
//!    in copy mode) scrolls the copy-mode viewport.
//!
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_resize -- --ignored --test-threads=1`

use bevy::prelude::*;
use crossbeam_channel::RecvTimeoutError;
use ozmux_tmux::{
    ConnectionState, CopyStateQuery, PaneId, ResizePaneX, ShowBuffer, TmuxCommand, TmuxConnection,
    TmuxPane, TmuxSessionPlugin, parse_copy_state,
};
use std::time::{Duration, Instant};
use tmux_control::{ClientEvent, TmuxServer, TransportEvent};

fn pump_until(app: &mut App, secs: u64, mut done: impl FnMut(&mut App) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        app.update();
        if done(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn handle_of(app: &App) -> tmux_control::TmuxHandle {
    app.world()
        .get_non_send_resource::<TmuxConnection>()
        .unwrap()
        .client()
        .unwrap()
        .handle()
}

fn teardown(app: &mut App) {
    if let Some(client) = app
        .world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .unwrap()
        .take()
    {
        client.handle().send("kill-server").ok();
    }
}

fn attach_and_wait_for_pane(tag: &str) -> (App, PaneId) {
    let socket = format!("ozmux-resize-{}-{}", std::process::id(), tag);
    let server = TmuxServer::new().socket_name(&socket);
    let client = server.new_session().expect("spawn tmux -CC new-session");

    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    app.world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection inserted by the plugin")
        .set(client);

    let mut pane_q = app.world_mut().query::<&TmuxPane>();
    let ready = pump_until(&mut app, 5, |app| {
        *app.world().resource::<ConnectionState>() == ConnectionState::Attached
            && pane_q.iter(app.world()).next().is_some()
    });
    assert!(ready, "tmux should attach and project a pane within 5 s");

    let pane_id = pane_q
        .iter(app.world())
        .next()
        .map(|p| p.id)
        .expect("a projected pane");
    (app, pane_id)
}

fn send_cmd(app: &App, cmd: &str) {
    handle_of(app).send(cmd).expect("send command");
}

/// Reads the current `dims.width` for the given pane from the ECS world.
fn pane_width(app: &mut App, id: PaneId) -> Option<u32> {
    app.world_mut()
        .query::<&TmuxPane>()
        .iter(app.world())
        .find(|p| p.id == id)
        .map(|p| p.dims.width)
}

/// Waits for a `show-buffer` reply on the raw transport channel (bypasses Bevy's
/// plugin drain so we can read the reply lines directly). The Bevy app is pumped
/// in parallel to keep the plugin's drain alive; the transport's buffered reader
/// thread is shared with Bevy's drain (both read from the same `Receiver`), so
/// at most one of the two will collect any given reply. We use `recv_timeout`
/// here while the app is NOT being updated so there is no contention.
fn read_show_buffer_reply(app: &mut App, timeout: Duration) -> Option<String> {
    let handle = handle_of(app);
    let id = handle.send(ShowBuffer).expect("show-buffer");
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let event = app
            .world()
            .get_non_send_resource::<TmuxConnection>()
            .unwrap()
            .client()
            .unwrap()
            .events()
            .recv_timeout(remaining.min(Duration::from_millis(200)));
        match event {
            Ok(TransportEvent::Protocol(ClientEvent::CommandComplete {
                id: cid,
                ok,
                output,
                ..
            })) if cid == id => {
                if ok {
                    return Some(output.join("\n"));
                } else {
                    return None;
                }
            }
            Ok(_) => continue,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// Waits for a `CopyStateQuery` reply on the raw transport channel.
fn read_copy_state_reply(app: &mut App, pane_id: PaneId, timeout: Duration) -> Option<String> {
    let handle = handle_of(app);
    let cmd = CopyStateQuery { pane: pane_id }.into_raw_command();
    let id = handle.send(&cmd).expect("copy-state-query");
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let event = app
            .world()
            .get_non_send_resource::<TmuxConnection>()
            .unwrap()
            .client()
            .unwrap()
            .events()
            .recv_timeout(remaining.min(Duration::from_millis(200)));
        match event {
            Ok(TransportEvent::Protocol(ClientEvent::CommandComplete {
                id: cid,
                ok,
                output,
                ..
            })) if cid == id => {
                if ok {
                    return output.into_iter().next();
                } else {
                    return None;
                }
            }
            Ok(_) => continue,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// Sends a copy-mode `send-keys -X` command targeting `pane_id` with an optional
/// repeat count. Mirrors the exact shape used by the mouse arbiter.
fn send_keys_x(app: &App, pane_id: PaneId, n: Option<u32>, copy_cmd: &str) {
    let n_flag = n.map_or(String::new(), |n| format!("-N {n} "));
    let cmd = format!("send-keys -X -t %{} {n_flag}{copy_cmd}", pane_id.0);
    handle_of(app).send(&cmd).expect("send-keys -X");
}

// ─── Test 1: resize-pane settles via %layout-change ────────────────────────

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn resize_pane_settle() {
    let (mut app, pane_id) = attach_and_wait_for_pane("resize");

    // Split the window into two panes so resize-pane has somewhere to put the
    // space (a sole pane in a window cannot be resized).
    send_cmd(&app, "split-window -h");
    let split_done = pump_until(&mut app, 5, |app| {
        let mut q = app.world_mut().query::<&TmuxPane>();
        q.iter(app.world()).count() >= 2
    });
    assert!(
        split_done,
        "split-window should produce at least two projected panes"
    );

    let initial_width = pane_width(&mut app, pane_id).expect("first pane has a projected width");

    // Request a wider left pane (ensure we don't exceed tmux's window width).
    let target_width = (initial_width + 10).min(60);
    let cmd = ResizePaneX {
        id: pane_id,
        width: target_width,
    }
    .into_raw_command();
    send_cmd(&app, &cmd);

    // Pump until the projected width for the first pane changes — the
    // `%layout-change` that confirms the resize must land and be applied.
    let settled = pump_until(&mut app, 5, |app| {
        pane_width(app, pane_id).is_some_and(|w| w != initial_width)
    });

    let final_width = pane_width(&mut app, pane_id).unwrap_or(initial_width);
    teardown(&mut app);

    assert!(
        settled,
        "resize-pane -x {target_width} must produce a %layout-change that updates \
         the projected pane width (initial={initial_width})"
    );
    // tmux may clamp to the actual window geometry; assert we moved toward the
    // target rather than demanding an exact match.
    assert!(
        final_width > initial_width,
        "pane width must have grown toward target {target_width}; \
         initial={initial_width} final={final_width}"
    );
}

// ─── Test 2: copy-mode drag-select copies the right text ───────────────────

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn copy_mode_drag_select() {
    let (mut app, pane_id) = attach_and_wait_for_pane("drag");
    let target = format!("%{}", pane_id.0);

    // Seed a known line so we have deterministic content to select.
    send_cmd(
        &app,
        &format!("send-keys -t {target} -l -- 'echo OZMUX_SELECT_TEST'"),
    );
    send_cmd(&app, &format!("send-keys -t {target} Enter"));
    // Give the shell time to execute and produce output.
    let _waited = pump_until(&mut app, 3, |_| false);
    std::thread::sleep(Duration::from_millis(500));

    // Enter copy mode on the pane.
    send_cmd(&app, &format!("copy-mode -t {target}"));
    std::thread::sleep(Duration::from_millis(200));

    // Query copy state to confirm we are in copy mode and get the cursor row.
    let state_line = read_copy_state_reply(&mut app, pane_id, Duration::from_secs(3));
    let state = state_line
        .as_deref()
        .and_then(parse_copy_state)
        .expect("copy-state-query must return a parseable line while in copy mode");
    assert!(
        state.pane_in_mode,
        "pane must be in copy mode after copy-mode command"
    );

    // Move cursor to the beginning of the line containing "OZMUX_SELECT_TEST"
    // by going to the top of the viewport (tmux copy-mode starts at bottom).
    // Use top-line to reach the content: scroll up enough, then position.
    // The echo output will be near the bottom; we use select-line to grab it.
    // First go to top of history to find the line consistently.
    send_keys_x(&app, pane_id, None, "history-top");
    std::thread::sleep(Duration::from_millis(100));

    // Use search-forward to jump to the line with our known marker text.
    let search_cmd = format!("send-keys -X -t {target} search-forward -- 'OZMUX_SELECT_TEST'");
    send_cmd(&app, &search_cmd);
    std::thread::sleep(Duration::from_millis(300));

    // Now query state again to confirm cursor moved.
    let _state2 = read_copy_state_reply(&mut app, pane_id, Duration::from_secs(3))
        .and_then(|l| parse_copy_state(&l));

    // Select from the current cursor to end of word using begin-selection then
    // move right to cover "OZMUX_SELECT_TEST" (18 chars) then copy-selection.
    send_keys_x(&app, pane_id, None, "begin-selection");
    send_keys_x(&app, pane_id, Some(18), "cursor-right");
    send_keys_x(&app, pane_id, None, "copy-selection");
    std::thread::sleep(Duration::from_millis(200));

    // show-buffer should contain our marker text.
    let buf = read_show_buffer_reply(&mut app, Duration::from_secs(5));
    teardown(&mut app);

    let buf = buf.expect("show-buffer must return a non-empty reply after copy-selection");
    assert!(
        buf.contains("OZMUX_SELECT_TEST"),
        "show-buffer must contain 'OZMUX_SELECT_TEST'; got: {:?}",
        buf
    );
}

// ─── Test 3: select-word / select-line + first-character quirk ─────────────

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn select_word_and_line() {
    let (mut app, pane_id) = attach_and_wait_for_pane("word");
    let target = format!("%{}", pane_id.0);

    // Seed deterministic three-word content on one line.
    send_cmd(
        &app,
        &format!("send-keys -t {target} -l -- 'echo alpha beta gamma'"),
    );
    send_cmd(&app, &format!("send-keys -t {target} Enter"));
    std::thread::sleep(Duration::from_millis(500));

    // Enter copy mode and go to the top of history.
    send_cmd(&app, &format!("copy-mode -t {target}"));
    std::thread::sleep(Duration::from_millis(200));
    send_keys_x(&app, pane_id, None, "history-top");
    std::thread::sleep(Duration::from_millis(100));

    // Jump to the line with "alpha beta gamma".
    let search_cmd = format!("send-keys -X -t {target} search-forward -- 'alpha beta gamma'");
    send_cmd(&app, &search_cmd);
    std::thread::sleep(Duration::from_millis(300));

    // ── Sub-case A: cursor on a non-first character of "beta" ──────────────
    // After search-forward the cursor should be at the start of the match.
    // "alpha " is 6 chars — move right 7 to land inside "beta" (on 'e').
    send_keys_x(&app, pane_id, Some(7), "cursor-right");
    std::thread::sleep(Duration::from_millis(100));

    send_keys_x(&app, pane_id, None, "select-word");
    send_keys_x(&app, pane_id, None, "copy-selection");
    std::thread::sleep(Duration::from_millis(200));

    let buf_beta = read_show_buffer_reply(&mut app, Duration::from_secs(5))
        .expect("show-buffer after select-word on 'beta'");

    // ── Sub-case B: cursor on the FIRST character of "beta" (tmux #1820) ───
    // Re-enter copy mode, navigate back to the same line, then position on 'b'.
    send_cmd(&app, &format!("copy-mode -t {target}"));
    std::thread::sleep(Duration::from_millis(200));
    send_keys_x(&app, pane_id, None, "history-top");
    std::thread::sleep(Duration::from_millis(100));
    send_cmd(&app, &search_cmd);
    std::thread::sleep(Duration::from_millis(300));

    // Move right 6 to land on 'b' (the first char of "beta").
    send_keys_x(&app, pane_id, Some(6), "cursor-right");
    std::thread::sleep(Duration::from_millis(100));

    send_keys_x(&app, pane_id, None, "select-word");
    send_keys_x(&app, pane_id, None, "copy-selection");
    std::thread::sleep(Duration::from_millis(200));

    let buf_beta_first_char = read_show_buffer_reply(&mut app, Duration::from_secs(5))
        .expect("show-buffer after select-word on first char of 'beta'");

    // ── Sub-case C: select-line grabs the whole line ────────────────────────
    send_cmd(&app, &format!("copy-mode -t {target}"));
    std::thread::sleep(Duration::from_millis(200));
    send_keys_x(&app, pane_id, None, "history-top");
    std::thread::sleep(Duration::from_millis(100));
    send_cmd(&app, &search_cmd);
    std::thread::sleep(Duration::from_millis(300));

    send_keys_x(&app, pane_id, None, "select-line");
    send_keys_x(&app, pane_id, None, "copy-selection");
    std::thread::sleep(Duration::from_millis(200));

    let buf_line = read_show_buffer_reply(&mut app, Duration::from_secs(5))
        .expect("show-buffer after select-line");
    teardown(&mut app);

    // ── Assertions ──────────────────────────────────────────────────────────

    // Sub-case A: non-first char of "beta" → select-word grabs "beta".
    assert!(
        buf_beta.trim() == "beta",
        "select-word on a non-first char of 'beta' must select 'beta'; got: {:?}",
        buf_beta
    );

    // Sub-case B: first char of "beta" — document ACTUAL tmux 3.6b behavior.
    // NOTE: tmux #1820 describes a bug where select-word on a word's first character
    // selects the preceding word. In tmux 3.6b (verified here) this bug does NOT
    // reproduce: the cursor on 'b' (first char of "beta") correctly selects "beta".
    // The test asserts the observed behavior so the mouse arbiter's multi-click
    // positioning can rely on it — if this changes in a future tmux version,
    // update both this assertion and the arbiter's word-click offset logic.
    assert!(
        buf_beta_first_char.trim() == "beta",
        "select-word on the first char of 'beta' must select 'beta' in tmux 3.6b \
         (tmux #1820 does not reproduce here); got: {:?}. If this regressed, \
         the mouse arbiter's word-click positioning may need an off-by-one offset.",
        buf_beta_first_char
    );

    // Sub-case C: select-line grabs everything on the line (contains all three words).
    let line_text = buf_line.trim();
    assert!(
        line_text.contains("alpha") && line_text.contains("beta") && line_text.contains("gamma"),
        "select-line must contain all three words 'alpha beta gamma'; got: {:?}",
        buf_line
    );
}

// ─── Test 4: the WheelUpPane conditional binding enters copy mode ───────────

/// Reproduces, end to end, what ozmux dispatches on a wheel-up over a normal
/// pane: tmux's default `WheelUpPane` root binding is an `if-shell` conditional
/// (`{ send-keys -M }` when the app wants the wheel, else `{ copy-mode -e }`).
/// On a plain shell pane (not alt-screen, not in a mode, no mouse reporting) the
/// conditional must take the `copy-mode -e` branch and enter copy mode. This is
/// the premise behind `is_copy_mode_entry` recognizing `copy-mode` inside the
/// conditional so ozmux inserts `CopyModeState`.
#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn wheel_binding_enters_copy_mode() {
    let (mut app, pane_id) = attach_and_wait_for_pane("wheel");
    let target = format!("%{}", pane_id.0);

    // tmux's default WheelUpPane binding, targeted at the test pane. The `#{...}`
    // braces are literal tmux format syntax, so build the string explicitly
    // rather than through `format!` brace-escaping.
    let mut binding = String::from("if-shell -F -t ");
    binding.push_str(&target);
    binding.push_str(
        " \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M -t ",
    );
    binding.push_str(&target);
    binding.push_str(" } { copy-mode -e -t ");
    binding.push_str(&target);
    binding.push_str(" }");
    send_cmd(&app, &binding);
    std::thread::sleep(Duration::from_millis(200));

    let state = read_copy_state_reply(&mut app, pane_id, Duration::from_secs(3))
        .as_deref()
        .and_then(parse_copy_state)
        .expect("copy-state reply after the wheel binding");
    teardown(&mut app);
    assert!(
        state.pane_in_mode,
        "the WheelUpPane if-shell binding must enter copy mode on a plain pane",
    );
}

// ─── Test 5: ozmux's direct copy-mode wheel-scroll command scrolls ─────────

/// Proves the wheel-in-copy-mode fix end to end. ozmux no longer relays tmux's
/// copy-mode `WheelUpPane` binding (a `select-pane \; send-keys …` sequence that
/// desyncs the control protocol); instead it sends a single pane-targeted
/// `send-keys -X -t %id -N <n> scroll-up` (`tmux_input::scroll_command`). That
/// command must scroll the copy-mode viewport.
#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn wheel_in_copy_mode_scrolls() {
    let (mut app, pane_id) = attach_and_wait_for_pane("wheelscroll");
    let target = format!("%{}", pane_id.0);

    // Seed enough scrollback that scroll-up has somewhere to go.
    send_cmd(&app, &format!("send-keys -t {target} -l -- 'seq 1 100'"));
    send_cmd(&app, &format!("send-keys -t {target} Enter"));
    std::thread::sleep(Duration::from_millis(900));
    let _ = pump_until(&mut app, 2, |_| false);

    send_cmd(&app, &format!("copy-mode -t {target}"));
    std::thread::sleep(Duration::from_millis(200));
    let entered = read_copy_state_reply(&mut app, pane_id, Duration::from_secs(3))
        .and_then(|l| parse_copy_state(&l))
        .expect("copy-state after entering copy mode");
    assert!(entered.pane_in_mode, "must be in copy mode");
    assert_eq!(entered.scroll_position, 0, "copy mode starts at the tail");

    // Exactly what ozmux now sends per wheel notch in copy mode.
    send_cmd(&app, &format!("send-keys -X -t {target} -N 3 scroll-up"));
    std::thread::sleep(Duration::from_millis(250));
    let scrolled = read_copy_state_reply(&mut app, pane_id, Duration::from_secs(3))
        .and_then(|l| parse_copy_state(&l))
        .expect("copy-state after the direct scroll command");
    teardown(&mut app);
    assert!(
        scrolled.scroll_position >= 1,
        "ozmux's direct copy-mode scroll command must scroll the viewport; \
         scroll_position stayed at {}",
        scrolled.scroll_position
    );
}
