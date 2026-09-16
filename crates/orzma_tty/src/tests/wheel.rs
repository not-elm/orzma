//! Tests for `send_wheel`: the route each frame takes by the VT's
//! modes, and the bytes that leave in one PTY write.

use super::*;

/// A wheel frame of `up` vertical and `right` horizontal notches over
/// cell (6, 4), with no modifiers held.
fn wheel(up: i32, right: i32) -> WheelInput {
    WheelInput {
        up,
        right,
        mods: WheelModifiers::default(),
        cell: Some(CellCoord { col: 6, row: 4 }),
        report_mods: ProtocolModifiers::default(),
    }
}

/// Asserts that `send_wheel` over a tracking terminal writes one
/// wheel-up report per notch, all in one write, and scrolls nothing.
///
/// Case: nvim runs with `mouse=nvi`, and the user spins the wheel up
/// two notches over its buffer.
#[test]
fn send_wheel_over_a_tracking_terminal_writes_a_report_per_notch() {
    let (mut term, sink) = tracking_term();
    term.send_wheel(wheel(2, 0), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(sink.contents(), b"\x1b[<64;6;4M\x1b[<64;6;4M");
    assert_eq!(sink.writes(), 1);
    assert!(term.vt.scrolls.is_empty());
}

/// Asserts that a wheel report carries the modifier bits the host
/// gathered.
///
/// Case: the user holds Option while spinning the wheel over nvim.
#[test]
fn send_wheel_reports_carry_the_gathered_modifier_bits() {
    let (mut term, sink) = tracking_term();
    let input = WheelInput {
        report_mods: ProtocolModifiers {
            alt: true,
            ..ProtocolModifiers::default()
        },
        ..wheel(1, 0)
    };
    term.send_wheel(input, &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(sink.contents(), b"\x1b[<72;6;4M");
}

/// Asserts that a tracking terminal writes nothing for notches
/// gathered with no cell under the cursor.
///
/// Case: a client of the multiplexer's command channel that does not
/// hit-test the cursor sends wheel notches to a pane nvim is tracking
/// the mouse in, naming no cell.
#[test]
fn send_wheel_drops_reports_without_a_cell() {
    let (mut term, sink) = tracking_term();
    let input = WheelInput {
        cell: None,
        ..wheel(2, 0)
    };
    term.send_wheel(input, &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(sink.contents(), b"");
}

/// Asserts that `send_wheel` over the alternate screen without
/// tracking writes the notches' lines as cursor keys in one write,
/// honouring DECCKM.
///
/// Case: `less` shows a long file on the alternate screen, and the
/// user spins the wheel up two notches; then the same inside an
/// application that also set application cursor keys.
#[test]
fn send_wheel_over_the_alternate_screen_writes_cursor_keys() {
    let (mut term, sink) = detached_term();
    term.vt.modes.active_screen = ScreenKind::Alternate;
    term.send_wheel(wheel(2, 0), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(sink.contents(), b"\x1b[A".repeat(6));

    let (mut term, sink) = detached_term();
    term.vt.modes.active_screen = ScreenKind::Alternate;
    term.vt.modes.app_cursor = true;
    term.send_wheel(wheel(1, 0), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(sink.contents(), b"\x1bOA".repeat(3));
}

/// Asserts that Shift over a tracking alternate screen skips the
/// reports and writes cursor keys instead.
///
/// Case: the user holds Shift and scrolls over nvim, whose mouse
/// tracking would otherwise take the wheel.
#[test]
fn send_wheel_with_shift_over_a_tracking_alternate_screen_writes_cursor_keys() {
    let (mut term, sink) = tracking_term();
    term.vt.modes.active_screen = ScreenKind::Alternate;
    let input = WheelInput {
        mods: WheelModifiers {
            shift: true,
            fine: false,
        },
        ..wheel(2, 0)
    };
    term.send_wheel(input, &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(sink.contents(), b"\x1b[A".repeat(6));
}

/// Asserts that horizontal notches become wheel-left / wheel-right
/// reports over a tracking terminal and nothing otherwise.
///
/// Case: the user swipes a trackpad sideways over nvim, then over a
/// shell prompt.
#[test]
fn send_wheel_routes_horizontal_notches_to_reports_only() {
    let (mut term, sink) = tracking_term();
    term.send_wheel(wheel(0, 1), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(sink.contents(), b"\x1b[<67;6;4M");

    let (mut term, sink) = tracking_term();
    term.send_wheel(wheel(0, -1), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(sink.contents(), b"\x1b[<66;6;4M");

    let (mut term, sink) = detached_term();
    term.send_wheel(wheel(0, 1), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(sink.contents(), b"");
    assert!(term.vt.scrolls.is_empty());
}

/// Asserts that `send_wheel` on the primary screen without tracking
/// scrolls the viewport by the notches' lines, writes nothing, and
/// arms a repaint.
///
/// Case: the user spins the wheel up two notches at a shell prompt
/// with output in scrollback.
#[test]
fn send_wheel_on_the_primary_screen_scrolls_the_viewport() {
    let (mut term, sink) = detached_term();
    term.vt.scroll_moves = true;
    term.send_wheel(wheel(2, 0), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(term.vt.scrolls, vec![Scroll::Delta(6)]);
    assert_eq!(sink.contents(), b"");
    assert!(term.coalescer.is_armed());
}

/// Asserts that a frame carrying both axes over a tracking terminal
/// goes out as one write with the vertical reports first.
///
/// Case: a diagonal trackpad swipe over nvim with the axis lock
/// disabled leaves one notch on each axis.
#[test]
fn send_wheel_writes_both_axes_in_one_write_vertical_first() {
    let (mut term, sink) = tracking_term();
    term.send_wheel(wheel(1, 1), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(sink.contents(), b"\x1b[<64;6;4M\x1b[<67;6;4M");
    assert_eq!(sink.writes(), 1);
}

/// Asserts that the cursor-key route snaps a scrolled-back viewport to
/// the live tail before writing, while the report route leaves the
/// viewport where it is.
///
/// Case: the user scrolls back through a pane's history, then spins the
/// wheel over `less` on the alternate screen; later they do the same over
/// nvim, whose mouse tracking takes the wheel.
#[test]
fn send_wheel_snaps_the_viewport_only_on_the_cursor_key_route() {
    let (mut term, sink) = detached_term();
    term.vt.modes.active_screen = ScreenKind::Alternate;
    term.vt.display_offset = DisplayOffset(5);
    term.send_wheel(wheel(1, 0), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(term.vt.display_offset, DisplayOffset(0));
    assert_eq!(sink.contents(), b"\x1b[A".repeat(3));

    let (mut term, sink) = tracking_term();
    term.vt.display_offset = DisplayOffset(5);
    term.send_wheel(wheel(1, 0), &WheelConfig::default())
        .expect("send_wheel");
    assert_eq!(term.vt.display_offset, DisplayOffset(5));
    assert_eq!(sink.contents(), b"\x1b[<64;6;4M");
}

/// Asserts that a PTY write failure surfaces as `PtyWrite`.
///
/// Case: the shell's PTY closed under the wheel gesture.
#[test]
fn send_wheel_reports_a_pty_write_failure() {
    let mut term = OrzmaTty::detached(
        FakeVt::new(grid(80, 24)),
        grid(80, 24),
        Box::new(FailingSink),
    )
    .expect("OrzmaTty::detached");
    term.vt.modes.mouse_tracking = MouseTracking::Drag;
    assert!(matches!(
        term.send_wheel(wheel(1, 0), &WheelConfig::default()),
        Err(OrzmaTtyError::PtyWrite(_))
    ));
}
