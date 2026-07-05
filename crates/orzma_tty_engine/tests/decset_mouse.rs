//! Unit test for the DECSET mouse-mode premise:
//!
//! A detached `TerminalHandle` that receives raw DECSET bytes
//! (`\x1b[?1000h` / `\x1b[?1006h`) via `advance()` must reflect
//! `MOUSE_REPORT_CLICK` and `SGR_MOUSE` in `current_modes()`.
//!
//! This closes the `[unverified]` premise in the arbiter design doc:
//! the handle side works independently of tmux, so the only remaining
//! question is whether tmux forwards these bytes through `%output`
//! (verified by `real_tmux_mouse::decset_bytes_reach_pane_output` in
//! `crates/tmux_session/tests/`).

use orzma_tty_engine::{TermMode, TerminalHandle};

/// Feeding `\x1b[?1000h` (X10 mouse click) + `\x1b[?1006h` (SGR mouse)
/// to a detached `TerminalHandle` via `advance()` must activate
/// `MOUSE_REPORT_CLICK` and `SGR_MOUSE` in `current_modes()`.
#[test]
fn decset_mouse_modes_reflect_in_current_modes() {
    let mut handle = TerminalHandle::detached(80, 24);

    // ESC [ ? 1000 h → mouse report click; ESC [ ? 1006 h → SGR extended mouse.
    let decset = b"\x1b[?1000h\x1b[?1006h";
    handle.advance(decset);

    let modes = handle.current_modes();
    assert!(
        modes.contains(TermMode::MOUSE_REPORT_CLICK),
        "MOUSE_REPORT_CLICK must be set after advancing \\x1b[?1000h; modes={modes:?}"
    );
    assert!(
        modes.contains(TermMode::SGR_MOUSE),
        "SGR_MOUSE must be set after advancing \\x1b[?1006h; modes={modes:?}"
    );
}

/// Resetting mouse modes via DECRST (`\x1b[?1000l` / `\x1b[?1006l`) must
/// clear the corresponding flags, confirming round-trip mode tracking.
#[test]
fn decrst_clears_mouse_modes() {
    let mut handle = TerminalHandle::detached(80, 24);

    handle.advance(b"\x1b[?1000h\x1b[?1006h");
    let after_set = handle.current_modes();
    assert!(after_set.contains(TermMode::MOUSE_REPORT_CLICK));
    assert!(after_set.contains(TermMode::SGR_MOUSE));

    handle.advance(b"\x1b[?1000l\x1b[?1006l");
    let after_reset = handle.current_modes();
    assert!(
        !after_reset.contains(TermMode::MOUSE_REPORT_CLICK),
        "MOUSE_REPORT_CLICK must be cleared after \\x1b[?1000l; modes={after_reset:?}"
    );
    assert!(
        !after_reset.contains(TermMode::SGR_MOUSE),
        "SGR_MOUSE must be cleared after \\x1b[?1006l; modes={after_reset:?}"
    );
}
