//! Tests that the eighth-bit meta mode is ignored, so the key encoder's
//! ESC prefix stays the only Alt encoding.

use super::*;
use crate::device::modes::VtModes;

/// Asserts that `CSI ? 1034 h` and `CSI ? 1034 l` leave every mode at
/// its default rather than recording a meta encoding.
///
/// Case: bash starts readline on an `xterm-256color` terminal, and
/// readline sends the terminfo `smm` string to turn the meta key on.
#[test]
fn the_meta_key_mode_changes_no_mode() {
    for request in [&b"\x1b[?1034h"[..], b"\x1b[?1034l"] {
        assert_eq!(
            interpret(request).modes(),
            VtModes::default(),
            "{request:?} changes no mode"
        );
    }
}

/// Asserts that neither setting nor resetting the meta key mode raises
/// chunk liveness, a reply, or a signal.
///
/// Case: a shell turns the meta key on as it starts a prompt and off
/// again as it hands the terminal to a program.
#[test]
fn neither_setting_nor_resetting_the_meta_key_mode_raises_anything() {
    for request in [&b"\x1b[?1034h"[..], b"\x1b[?1034l"] {
        let output = interpret_fully(request).1;
        assert!(!output.damaged, "{request:?} raises no liveness");
        assert!(output.replies.is_empty(), "{request:?} sends no reply");
        assert!(output.signals.is_empty(), "{request:?} raises no signal");
    }
}
