//! Tests for [`VtBackend::palette`]: OSC overrides folded over the
//! xterm defaults.

use super::*;
use crate::schema::Rgb;

/// Asserts that an OSC 4 override reaches the published palette.
///
/// Case: a theming tool recolors palette slot 1, and the next frame's
/// resolution table must carry the new value.
#[test]
fn an_osc4_override_reaches_the_palette() {
    let vt = vt_after(b"\x1b]4;1;rgb:ff/00/00\x07");
    assert_eq!(vt.palette().indexed[1], Rgb { r: 255, g: 0, b: 0 });
}

/// Asserts that an OSC 104 reset restores the xterm default for the
/// slot.
///
/// Case: a theming tool undoes its override on exit, and indexed cells
/// must fall back to the stock xterm color rather than keep the stale
/// theme value.
#[test]
fn an_osc104_reset_restores_the_xterm_default() {
    let vt = vt_after(b"\x1b]4;1;rgb:ff/00/00\x07\x1b]104;1\x07");
    assert_eq!(vt.palette().indexed[1], Rgb { r: 205, g: 0, b: 0 });
}

/// Asserts that OSC 10 and OSC 11 recolor the palette's default
/// foreground and background.
///
/// Case: a theme switcher restyles the terminal defaults, so cells
/// painted with `SGR 39` / `SGR 49` must resolve against the new
/// colors on the next repaint.
#[test]
fn osc10_and_osc11_recolor_the_default_foreground_and_background() {
    let vt = vt_after(b"\x1b]10;rgb:aa/bb/cc\x07\x1b]11;rgb:11/22/33\x07");
    let palette = vt.palette();
    assert_eq!(
        palette.foreground,
        Rgb {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc
        }
    );
    assert_eq!(
        palette.background,
        Rgb {
            r: 0x11,
            g: 0x22,
            b: 0x33
        }
    );
}
