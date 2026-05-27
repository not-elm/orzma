//! Pure mouse-button routing for the Bevy terminal.
//!
//! This module is Bevy- and PTY-agnostic. The Bevy
//! `dispatch_mouse_buttons` system hit-tests the cursor, tracks click
//! count, builds a `ButtonEvent`, and calls `ButtonAction::route` to
//! decide whether the event becomes a local selection mutation or
//! PTY-bound mouse-protocol bytes.
//!
//! See `docs/superpowers/specs/2026-05-27-mouse-selection-design.md`
//! for the full decision table.

use crate::mouse_encode::{CellCoord, ProtocolModifiers, encode_protocol_event};
use alacritty_terminal::index::Side;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;

/// Discrete event kinds the router understands. Press/Release are
/// transitions on a single button; Drag is "motion while a button is
/// held".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonEventKind {
    Press,
    Release,
    Drag,
}

/// Logical mouse button identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButtonKind {
    Left,
    Middle,
    Right,
}

/// One mouse-button event, projected into pane-relative cell coords.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonEvent {
    pub kind: ButtonEventKind,
    pub button: MouseButtonKind,
    pub cell: CellCoord,
    pub side: Side,
    /// 1, 2, or 3 — caller-tracked. Drag and Release ignore this.
    pub click_count: u8,
}

/// Subset of MouseConfig used by `ButtonAction::route`.
#[derive(Clone, Debug, Default)]
pub struct ButtonConfig {
    /// Hard cap on the number of PTY-bound reports emitted per route
    /// call. Mirrors `WheelConfig::max_protocol_events_per_frame`.
    pub max_protocol_events_per_frame: u32,
}

/// What `ButtonAction::route` decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ButtonAction {
    /// Nothing to do this event.
    Noop,
    /// Write these pre-encoded bytes to the PTY.
    WriteToPty(Vec<u8>),
    /// Drop any active local selection AND write these bytes to the
    /// PTY. Used on forwarded press events so a previous highlight does
    /// not visually persist past a click that goes to the app.
    ClearAndWriteToPty(Vec<u8>),
    /// A single-click left-press has occurred. The Bevy glue should
    /// arm a pending drag at `(cell, side)` of type `ty` (always
    /// `Simple` in current usage) and clear any pre-existing local
    /// selection, but NOT call `selection_start_at` yet. The selection
    /// is materialized lazily on the first `UpdateLocalSelection`
    /// whose cell differs from the armed `cell`.
    ArmDrag {
        ty: SelectionType,
        cell: CellCoord,
        side: Side,
    },
    /// Begin a new local selection of `ty` at `(cell, side)`.
    StartLocalSelection {
        ty: SelectionType,
        cell: CellCoord,
        side: Side,
    },
    /// Extend the current local selection's moving end to `(cell, side)`.
    UpdateLocalSelection { cell: CellCoord, side: Side },
    /// Drop the local selection without writing to the PTY.
    ClearLocalSelection,
}

impl ButtonAction {
    /// Pure decision function — see module doc and spec §4 decision
    /// table. The router is stateless: click counting and drag-state
    /// tracking live in the Bevy glue.
    pub fn route(
        modes: TermMode,
        evt: ButtonEvent,
        mods: ProtocolModifiers,
        cfg: &ButtonConfig,
    ) -> Self {
        let app_captured = modes.intersects(TermMode::MOUSE_MODE);
        // Shift bypasses app capture — the user is asking for local
        // selection even while an app has mouse mode on.
        let route_locally = !app_captured || mods.shift;

        if route_locally {
            return route_locally_branch(evt, mods);
        }

        // PTY-forward path (app has mouse capture and Shift is not held).
        let cb_base: u8 = match evt.button {
            MouseButtonKind::Left => 0,
            MouseButtonKind::Middle => 1,
            MouseButtonKind::Right => 2,
        };
        let motion = matches!(evt.kind, ButtonEventKind::Drag);
        let release = matches!(evt.kind, ButtonEventKind::Release);
        let bytes = encode_protocol_event(modes, cb_base, evt.cell, mods, motion, release);

        if cfg.max_protocol_events_per_frame == 0 {
            // Caller asked for a hard cap of zero — drop the event.
            return ButtonAction::Noop;
        }

        match evt.kind {
            ButtonEventKind::Press => ButtonAction::ClearAndWriteToPty(bytes),
            ButtonEventKind::Drag | ButtonEventKind::Release => ButtonAction::WriteToPty(bytes),
        }
    }
}

fn route_locally_branch(evt: ButtonEvent, mods: ProtocolModifiers) -> ButtonAction {
    match (evt.kind, evt.button) {
        (ButtonEventKind::Press, MouseButtonKind::Left) => {
            let ty = if mods.alt {
                SelectionType::Block
            } else {
                match evt.click_count {
                    1 => SelectionType::Simple,
                    2 => SelectionType::Semantic,
                    _ => SelectionType::Lines,
                }
            };
            ButtonAction::StartLocalSelection {
                ty,
                cell: evt.cell,
                side: evt.side,
            }
        }
        (ButtonEventKind::Drag, MouseButtonKind::Left) => ButtonAction::UpdateLocalSelection {
            cell: evt.cell,
            side: evt.side,
        },
        // Release on local path: selection persists; nothing to do.
        (ButtonEventKind::Release, MouseButtonKind::Left) => ButtonAction::Noop,
        // Middle/Right buttons do nothing locally (no primary-selection paste).
        _ => ButtonAction::Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::Side;

    fn cell(col: u32, row: u32) -> CellCoord {
        CellCoord { col, row }
    }

    fn press_left(count: u8) -> ButtonEvent {
        ButtonEvent {
            kind: ButtonEventKind::Press,
            button: MouseButtonKind::Left,
            cell: cell(5, 5),
            side: Side::Left,
            click_count: count,
        }
    }

    fn cfg() -> ButtonConfig {
        ButtonConfig {
            max_protocol_events_per_frame: 8,
        }
    }

    #[test]
    fn left_press_single_click_starts_simple_selection() {
        let action = ButtonAction::route(
            TermMode::empty(),
            press_left(1),
            ProtocolModifiers::default(),
            &cfg(),
        );
        assert_eq!(
            action,
            ButtonAction::StartLocalSelection {
                ty: SelectionType::Simple,
                cell: cell(5, 5),
                side: Side::Left,
            }
        );
    }

    #[test]
    fn left_press_double_click_starts_semantic_selection() {
        let action = ButtonAction::route(
            TermMode::empty(),
            press_left(2),
            ProtocolModifiers::default(),
            &cfg(),
        );
        assert!(matches!(
            action,
            ButtonAction::StartLocalSelection {
                ty: SelectionType::Semantic,
                ..
            }
        ));
    }

    #[test]
    fn left_press_triple_click_starts_lines_selection() {
        let action = ButtonAction::route(
            TermMode::empty(),
            press_left(3),
            ProtocolModifiers::default(),
            &cfg(),
        );
        assert!(matches!(
            action,
            ButtonAction::StartLocalSelection {
                ty: SelectionType::Lines,
                ..
            }
        ));
    }

    #[test]
    fn left_press_with_alt_starts_block_selection_regardless_of_count() {
        let mods = ProtocolModifiers {
            alt: true,
            ..Default::default()
        };
        for count in [1, 2, 3] {
            let action = ButtonAction::route(TermMode::empty(), press_left(count), mods, &cfg());
            assert!(
                matches!(
                    action,
                    ButtonAction::StartLocalSelection {
                        ty: SelectionType::Block,
                        ..
                    }
                ),
                "click_count={} should still produce Block when Alt held",
                count
            );
        }
    }

    #[test]
    fn left_drag_updates_local_selection() {
        let evt = ButtonEvent {
            kind: ButtonEventKind::Drag,
            button: MouseButtonKind::Left,
            cell: cell(10, 10),
            side: Side::Right,
            click_count: 1,
        };
        let action =
            ButtonAction::route(TermMode::empty(), evt, ProtocolModifiers::default(), &cfg());
        assert_eq!(
            action,
            ButtonAction::UpdateLocalSelection {
                cell: cell(10, 10),
                side: Side::Right,
            }
        );
    }

    #[test]
    fn left_release_outside_capture_is_noop() {
        let evt = ButtonEvent {
            kind: ButtonEventKind::Release,
            button: MouseButtonKind::Left,
            cell: cell(5, 5),
            side: Side::Left,
            click_count: 1,
        };
        let action =
            ButtonAction::route(TermMode::empty(), evt, ProtocolModifiers::default(), &cfg());
        assert_eq!(action, ButtonAction::Noop);
    }

    #[test]
    fn middle_or_right_outside_capture_is_noop() {
        for button in [MouseButtonKind::Middle, MouseButtonKind::Right] {
            let evt = ButtonEvent {
                kind: ButtonEventKind::Press,
                button,
                cell: cell(1, 1),
                side: Side::Left,
                click_count: 1,
            };
            let action =
                ButtonAction::route(TermMode::empty(), evt, ProtocolModifiers::default(), &cfg());
            assert_eq!(action, ButtonAction::Noop, "button = {:?}", button);
        }
    }

    #[test]
    fn captured_press_emits_clear_and_write() {
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let action =
            ButtonAction::route(modes, press_left(1), ProtocolModifiers::default(), &cfg());
        match action {
            ButtonAction::ClearAndWriteToPty(bytes) => {
                // SGR left-press at (5, 5), no modifiers, no motion, not release.
                assert_eq!(bytes, b"\x1b[<0;5;5M");
            }
            other => panic!("expected ClearAndWriteToPty, got {:?}", other),
        }
    }

    #[test]
    fn captured_drag_emits_write_with_motion_bit() {
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let evt = ButtonEvent {
            kind: ButtonEventKind::Drag,
            button: MouseButtonKind::Left,
            cell: cell(5, 5),
            side: Side::Left,
            click_count: 1,
        };
        let action = ButtonAction::route(modes, evt, ProtocolModifiers::default(), &cfg());
        // Motion bit (32) is set; left-button base is 0; cb = 32.
        assert_eq!(action, ButtonAction::WriteToPty(b"\x1b[<32;5;5M".to_vec()));
    }

    #[test]
    fn captured_release_uses_lowercase_m_in_sgr() {
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let evt = ButtonEvent {
            kind: ButtonEventKind::Release,
            button: MouseButtonKind::Left,
            cell: cell(5, 5),
            side: Side::Left,
            click_count: 1,
        };
        let action = ButtonAction::route(modes, evt, ProtocolModifiers::default(), &cfg());
        assert_eq!(action, ButtonAction::WriteToPty(b"\x1b[<0;5;5m".to_vec()));
    }

    #[test]
    fn captured_middle_button_press_forwards_with_cb_1() {
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let evt = ButtonEvent {
            kind: ButtonEventKind::Press,
            button: MouseButtonKind::Middle,
            cell: cell(5, 5),
            side: Side::Left,
            click_count: 1,
        };
        let action = ButtonAction::route(modes, evt, ProtocolModifiers::default(), &cfg());
        match action {
            ButtonAction::ClearAndWriteToPty(bytes) => {
                assert_eq!(bytes, b"\x1b[<1;5;5M");
            }
            other => panic!(
                "expected ClearAndWriteToPty for middle press, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn shift_bypass_routes_locally_even_when_captured() {
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let mods = ProtocolModifiers {
            shift: true,
            ..Default::default()
        };
        let action = ButtonAction::route(modes, press_left(1), mods, &cfg());
        assert_eq!(
            action,
            ButtonAction::StartLocalSelection {
                ty: SelectionType::Simple,
                cell: cell(5, 5),
                side: Side::Left,
            }
        );
    }
}
