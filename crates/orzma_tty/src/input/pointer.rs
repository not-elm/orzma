//! Pointer routing: decides, from the modes the application set, whether
//! each press, motion, and release becomes a mouse report or drives the
//! terminal's own selection.

use crate::input::mouse::{
    CellCoord, MouseButton, MouseReport, MouseReportKind, ProtocolModifiers,
};
use orzma_vt::prelude::{CellSide, MouseTracking, SelectionKind, VtModes};

/// One pointer event the host UI hands to a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerInput {
    /// What happened.
    pub kind: PointerKind,
    /// The pressed or released button; `None` for [`PointerKind::Motion`]
    /// and [`PointerKind::Cancel`], which ignore it.
    pub button: Option<PointerButton>,
    /// The 1-based viewport cell under the pointer.
    /// [`PointerKind::Cancel`] ignores it.
    pub cell: CellCoord,
    /// Which half of the cell the pointer is in.
    pub side: CellSide,
    /// 1, 2, or 3 for a press, counting consecutive clicks; ignored
    /// otherwise. Zero counts as 1 and anything above 3 as 3.
    pub click_count: u8,
    /// The modifiers held with the event.
    pub mods: ProtocolModifiers,
}

/// What a [`PointerInput`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    /// A button went down.
    Press,
    /// The pointer moved to another cell.
    Motion,
    /// A button went up.
    Release,
    /// The host stopped tracking every held button without seeing it
    /// released.
    Cancel,
}

/// A physical pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    /// The primary button.
    Left,
    /// The middle button, or a wheel click.
    Middle,
    /// The secondary button.
    Right,
}

/// One effect of a routed pointer event, in the order it must apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PointerAction {
    /// A mouse report bound for the application.
    Report(MouseReport),
    /// Drops the active selection.
    SelectionClear,
    /// Anchors a new selection at a 1-based viewport cell.
    SelectionStart {
        cell: CellCoord,
        side: CellSide,
        kind: SelectionKind,
    },
    /// Moves the selection's moving end to a 1-based viewport cell.
    SelectionExtend { cell: CellCoord, side: CellSide },
    /// Hands the selected text to the clipboard.
    Copy,
}

/// A terminal's pointer state from each press to its release.
#[derive(Debug, Default)]
pub(crate) struct PointerState {
    /// Each button's routing, indexed Left / Middle / Right, fixed at its
    /// press and cleared at its release or a cancel.
    latches: [Option<Latch>; 3],
    /// The left button's selection gesture.
    local_drag: Option<LocalDrag>,
    /// The cell of the last report of any kind, for per-cell motion dedup.
    last_reported_cell: Option<CellCoord>,
    /// The last cell an event named, where a cancel reports its releases.
    last_input_cell: Option<CellCoord>,
    /// Whether the host's current run of consecutive clicks holds a
    /// forwarded press, so the local presses after it count their own
    /// clicks.
    forwarded_in_chain: bool,
    /// The click count the last local press was given.
    local_clicks: u8,
}

/// Where a held button's events go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Latch {
    /// To the application, as mouse reports.
    Forwarded,
    /// To the terminal's own selection.
    Local,
}

/// The left button's selection gesture, whose selection is anchored at
/// the press.
#[derive(Clone, Copy, Debug)]
struct LocalDrag {
    phase: DragPhase,
    /// The cell the selection's moving end was last put on.
    last_cell: CellCoord,
    /// The half of `last_cell` the moving end sits in.
    last_side: CellSide,
}

/// Whether a selection gesture has left the cell it was pressed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragPhase {
    /// A single click is held on the cell it was pressed on; its
    /// selection is still empty.
    Armed,
    /// The selection follows the pointer.
    Started,
}

impl PointerState {
    /// Routes one pointer event by the application's `modes` and returns
    /// the effects to apply, in order.
    ///
    /// `at_live_tail` is whether the viewport shows the live tail. A press
    /// is forwarded as a report only while a mouse tracking level is in
    /// force, Shift is not held, and the viewport is at the live tail;
    /// otherwise it drives the selection. That routing holds for the
    /// button until its release or a cancel, whatever the modes do
    /// meanwhile. A forwarded press clears the selection unless the left
    /// button is still dragging one, and a left-button selection is
    /// anchored at its press. Local presses that follow a forwarded press
    /// among consecutive clicks count their clicks from that press. A
    /// report is produced only at a tracking level that asks for it, a
    /// motion report only when the pointer left the cell of the last
    /// report, and a release whose press was not forwarded is never
    /// reported. An event routed while no tracking level is in force
    /// forgets the cell of the last report. The release of a left-button
    /// selection drag moves the selection's end to where it was released
    /// and yields [`PointerAction::Copy`], never together with a report.
    pub fn route(
        &mut self,
        input: PointerInput,
        modes: VtModes,
        at_live_tail: bool,
    ) -> Vec<PointerAction> {
        if !modes.mouse_reporting_active() {
            self.last_reported_cell = None;
        }
        let actions = match input.kind {
            PointerKind::Press => self.press(input, modes, at_live_tail),
            PointerKind::Motion => self.motion(input, modes, at_live_tail),
            PointerKind::Release => self.release(input, modes),
            PointerKind::Cancel => self.cancel(input, modes),
        };
        if input.kind != PointerKind::Cancel {
            self.last_input_cell = Some(input.cell);
        }
        actions
    }

    /// The 1-based viewport cell and half a held left-button selection
    /// drag last put the selection's moving end on; `None` unless such a
    /// drag has left the cell it was pressed on.
    pub fn drag_end(&self) -> Option<(CellCoord, CellSide)> {
        self.local_drag
            .filter(|drag| drag.phase == DragPhase::Started)
            .map(|drag| (drag.last_cell, drag.last_side))
    }

    fn press(
        &mut self,
        input: PointerInput,
        modes: VtModes,
        at_live_tail: bool,
    ) -> Vec<PointerAction> {
        let Some(button) = input.button else {
            return Vec::new();
        };
        let slot = button.index();
        let reporting = modes.mouse_reporting_active();
        let mut actions = Vec::new();
        if self.latches[slot] == Some(Latch::Forwarded) && reporting {
            actions.push(self.report(
                button.report_button(),
                MouseReportKind::Release,
                input.cell,
                input.mods,
            ));
        }
        if reporting && !input.mods.shift && at_live_tail {
            self.latches[slot] = Some(Latch::Forwarded);
            self.forwarded_in_chain = true;
            self.local_clicks = 0;
            if !self.left_still_selecting(button) {
                self.local_drag = None;
                actions.push(PointerAction::SelectionClear);
            }
            actions.push(self.report(
                button.report_button(),
                MouseReportKind::Press,
                input.cell,
                input.mods,
            ));
            return actions;
        }
        self.latches[slot] = Some(Latch::Local);
        let click_count = self.local_click_count(input.click_count);
        if button == PointerButton::Left {
            actions.push(self.start_local(input, click_count));
        }
        actions
    }

    /// Whether the left button, pressed before `button`, still drives a
    /// selection drag that a press of `button` must leave alone.
    fn left_still_selecting(&self, button: PointerButton) -> bool {
        button != PointerButton::Left
            && self.latches[PointerButton::Left.index()] == Some(Latch::Local)
    }

    /// The click count a local press counts as, given the host's count of
    /// consecutive clicks. A forwarded press earlier in the same run
    /// restarts the count, so only the local presses since it count.
    fn local_click_count(&mut self, host_clicks: u8) -> u8 {
        if host_clicks <= 1 {
            self.forwarded_in_chain = false;
        }
        let clicks = if self.forwarded_in_chain {
            self.local_clicks.saturating_add(1).min(3)
        } else {
            host_clicks.clamp(1, 3)
        };
        self.local_clicks = clicks;
        clicks
    }

    fn start_local(&mut self, input: PointerInput, click_count: u8) -> PointerAction {
        let (kind, phase) = if input.mods.alt {
            // TODO: switch to a block selection once `orzma_vt` gains one;
            // an Alt+click rounds down to `Lines` until then.
            (SelectionKind::Lines, DragPhase::Started)
        } else {
            match click_count {
                1 => (SelectionKind::Simple, DragPhase::Armed),
                // TODO: switch to a word-snapped kind once `orzma_vt` gains
                // one; a double-click rounds down to `Simple` until then.
                2 => (SelectionKind::Simple, DragPhase::Started),
                _ => (SelectionKind::Lines, DragPhase::Started),
            }
        };
        self.local_drag = Some(LocalDrag {
            phase,
            last_cell: input.cell,
            last_side: input.side,
        });
        PointerAction::SelectionStart {
            cell: input.cell,
            side: input.side,
            kind,
        }
    }

    fn motion(
        &mut self,
        input: PointerInput,
        modes: VtModes,
        at_live_tail: bool,
    ) -> Vec<PointerAction> {
        let moved = self.last_reported_cell != Some(input.cell);
        let mut actions = Vec::new();
        if let Some(button) = self.lowest_forwarded() {
            if moved
                && matches!(
                    modes.mouse_tracking,
                    MouseTracking::Drag | MouseTracking::Motion
                )
            {
                actions.push(self.report(
                    button.report_button(),
                    MouseReportKind::Motion,
                    input.cell,
                    input.mods,
                ));
            }
        } else if self.latches.iter().all(Option::is_none)
            && moved
            && modes.mouse_tracking == MouseTracking::Motion
            && at_live_tail
        {
            actions.push(self.report(
                MouseButton::None,
                MouseReportKind::Motion,
                input.cell,
                input.mods,
            ));
        }
        actions.extend(self.extend_local(input));
        actions
    }

    fn extend_local(&mut self, input: PointerInput) -> Option<PointerAction> {
        let drag = self.local_drag.as_mut()?;
        if input.cell == drag.last_cell {
            return None;
        }
        drag.phase = DragPhase::Started;
        drag.last_cell = input.cell;
        drag.last_side = input.side;
        Some(PointerAction::SelectionExtend {
            cell: input.cell,
            side: input.side,
        })
    }

    fn release(&mut self, input: PointerInput, modes: VtModes) -> Vec<PointerAction> {
        let Some(button) = input.button else {
            return Vec::new();
        };
        match self.latches[button.index()].take() {
            Some(Latch::Forwarded) if modes.mouse_reporting_active() => vec![self.report(
                button.report_button(),
                MouseReportKind::Release,
                input.cell,
                input.mods,
            )],
            Some(Latch::Local) if button == PointerButton::Left => match self.local_drag.take() {
                Some(LocalDrag {
                    phase: DragPhase::Started,
                    ..
                }) => vec![
                    PointerAction::SelectionExtend {
                        cell: input.cell,
                        side: input.side,
                    },
                    PointerAction::Copy,
                ],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn cancel(&mut self, input: PointerInput, modes: VtModes) -> Vec<PointerAction> {
        let cell = self.last_input_cell.unwrap_or(input.cell);
        let mut actions = Vec::new();
        for button in PointerButton::ALL {
            let forwarded = self.latches[button.index()].take() == Some(Latch::Forwarded);
            if forwarded && modes.mouse_reporting_active() {
                actions.push(self.report(
                    button.report_button(),
                    MouseReportKind::Release,
                    cell,
                    input.mods,
                ));
            }
        }
        self.local_drag = None;
        actions
    }

    fn lowest_forwarded(&self) -> Option<PointerButton> {
        PointerButton::ALL
            .into_iter()
            .find(|button| self.latches[button.index()] == Some(Latch::Forwarded))
    }

    fn report(
        &mut self,
        button: MouseButton,
        kind: MouseReportKind,
        cell: CellCoord,
        mods: ProtocolModifiers,
    ) -> PointerAction {
        self.last_reported_cell = Some(cell);
        PointerAction::Report(MouseReport {
            button,
            kind,
            cell,
            mods,
        })
    }
}

impl PointerButton {
    /// The button's zero-based position, lowest-numbered first: `Left` is
    /// 0, `Middle` is 1, and `Right` is 2.
    pub fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
        }
    }

    /// Every button, lowest-numbered first.
    const ALL: [Self; 3] = [Self::Left, Self::Middle, Self::Right];

    fn report_button(self) -> MouseButton {
        match self {
            Self::Left => MouseButton::Left,
            Self::Middle => MouseButton::Middle,
            Self::Right => MouseButton::Right,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: bool = true;

    fn modes(tracking: MouseTracking) -> VtModes {
        VtModes {
            mouse_tracking: tracking,
            ..VtModes::default()
        }
    }

    fn at(col: u32, row: u32) -> CellCoord {
        CellCoord { col, row }
    }

    fn event(kind: PointerKind, button: Option<PointerButton>, col: u32, row: u32) -> PointerInput {
        PointerInput {
            kind,
            button,
            cell: at(col, row),
            side: CellSide::Left,
            click_count: 1,
            mods: ProtocolModifiers::default(),
        }
    }

    fn press(button: PointerButton, col: u32, row: u32) -> PointerInput {
        event(PointerKind::Press, Some(button), col, row)
    }

    fn motion(col: u32, row: u32) -> PointerInput {
        event(PointerKind::Motion, None, col, row)
    }

    fn release(button: PointerButton, col: u32, row: u32) -> PointerInput {
        event(PointerKind::Release, Some(button), col, row)
    }

    fn cancel() -> PointerInput {
        event(PointerKind::Cancel, None, 1, 1)
    }

    fn with_shift(mut input: PointerInput) -> PointerInput {
        input.mods.shift = true;
        input
    }

    fn with_alt(mut input: PointerInput) -> PointerInput {
        input.mods.alt = true;
        input
    }

    fn with_clicks(mut input: PointerInput, click_count: u8) -> PointerInput {
        input.click_count = click_count;
        input
    }

    fn report(button: MouseButton, kind: MouseReportKind, col: u32, row: u32) -> PointerAction {
        PointerAction::Report(MouseReport {
            button,
            kind,
            cell: at(col, row),
            mods: ProtocolModifiers::default(),
        })
    }

    fn reports(actions: &[PointerAction]) -> usize {
        actions
            .iter()
            .filter(|action| matches!(action, PointerAction::Report(_)))
            .count()
    }

    fn start(col: u32, row: u32, kind: SelectionKind) -> PointerAction {
        PointerAction::SelectionStart {
            cell: at(col, row),
            side: CellSide::Left,
            kind,
        }
    }

    fn extend(col: u32, row: u32) -> PointerAction {
        PointerAction::SelectionExtend {
            cell: at(col, row),
            side: CellSide::Left,
        }
    }

    /// Asserts that a press is forwarded exactly when a tracking level is
    /// in force, Shift is not held, and the viewport is at the live tail.
    ///
    /// Case: the user clicks in panes running nvim, a plain shell, and
    /// `fzf --height` scrolled back into history, with and without Shift
    /// held.
    #[test]
    fn a_press_is_forwarded_only_with_tracking_no_shift_and_the_live_tail() {
        for tracking in [
            MouseTracking::Off,
            MouseTracking::Clicks,
            MouseTracking::Drag,
            MouseTracking::Motion,
        ] {
            for shift in [false, true] {
                for live in [true, false] {
                    let mut state = PointerState::default();
                    let mut input = press(PointerButton::Left, 3, 2);
                    input.mods.shift = shift;
                    let actions = state.route(input, modes(tracking), live);
                    let forwarded = tracking != MouseTracking::Off && !shift && live;
                    assert_eq!(
                        reports(&actions) == 1,
                        forwarded,
                        "{tracking:?} shift={shift} live={live}: {actions:?}"
                    );
                }
            }
        }
    }

    /// Asserts the tracking-level table: presses and releases are
    /// reported from 1000 up, motion with a forwarded button held from
    /// 1002 up, and buttonless motion only at 1003.
    ///
    /// Case: the user clicks, drags, and hovers over applications that
    /// asked for click-only, button-event, and any-event tracking.
    #[test]
    fn each_tracking_level_reports_what_it_asks_for() {
        for (tracking, drags, hovers) in [
            (MouseTracking::Clicks, false, false),
            (MouseTracking::Drag, true, false),
            (MouseTracking::Motion, true, true),
        ] {
            let mut state = PointerState::default();
            let m = modes(tracking);
            assert_eq!(
                state.route(press(PointerButton::Left, 1, 1), m, LIVE)[1],
                report(MouseButton::Left, MouseReportKind::Press, 1, 1)
            );
            let drag = state.route(motion(2, 1), m, LIVE);
            assert_eq!(
                drag == vec![report(MouseButton::Left, MouseReportKind::Motion, 2, 1)],
                drags,
                "{tracking:?}: {drag:?}"
            );
            assert_eq!(
                state.route(release(PointerButton::Left, 2, 1), m, LIVE),
                vec![report(MouseButton::Left, MouseReportKind::Release, 2, 1)]
            );
            let hover = state.route(motion(3, 1), m, LIVE);
            assert_eq!(
                hover == vec![report(MouseButton::None, MouseReportKind::Motion, 3, 1)],
                hovers,
                "{tracking:?}: {hover:?}"
            );
        }
    }

    /// Asserts that a selection drag begun while tracking was off stays a
    /// selection after the application turns tracking on, and its release
    /// copies rather than reporting.
    ///
    /// Case: the user starts dragging a selection at a shell prompt just
    /// as a script starts `fzf --height`, which turns on mouse tracking in
    /// the same pane.
    #[test]
    fn a_selection_drag_survives_tracking_turning_on() {
        let mut state = PointerState::default();
        let off = modes(MouseTracking::Off);
        state.route(press(PointerButton::Left, 1, 1), off, LIVE);
        state.route(motion(3, 1), off, LIVE);
        let on = modes(MouseTracking::Drag);
        assert_eq!(state.route(motion(4, 1), on, LIVE), vec![extend(4, 1)]);
        assert_eq!(
            state.route(release(PointerButton::Left, 4, 1), on, LIVE),
            vec![extend(4, 1), PointerAction::Copy]
        );
    }

    /// Asserts that the release of a forwarded press is dropped once the
    /// application turned tracking off, and starts no selection or copy.
    ///
    /// Case: the user holds the button in nvim while nvim quits and the
    /// shell prompt returns, then lets go.
    #[test]
    fn a_forwarded_release_is_dropped_after_tracking_turns_off() {
        let mut state = PointerState::default();
        state.route(
            press(PointerButton::Left, 2, 2),
            modes(MouseTracking::Drag),
            LIVE,
        );
        let off = modes(MouseTracking::Off);
        assert!(state.route(motion(3, 2), off, LIVE).is_empty());
        assert!(
            state
                .route(release(PointerButton::Left, 3, 2), off, LIVE)
                .is_empty()
        );
        assert_eq!(
            state.route(press(PointerButton::Left, 3, 2), off, LIVE),
            vec![start(3, 2, SelectionKind::Simple)]
        );
    }

    /// Asserts that a release or a motion with no matching press is
    /// dropped.
    ///
    /// Case: a press opened a hyperlink instead of reaching the pane, and
    /// its release arrives over nvim.
    #[test]
    fn an_unmatched_release_is_dropped() {
        let mut state = PointerState::default();
        let m = modes(MouseTracking::Drag);
        assert!(
            state
                .route(release(PointerButton::Left, 1, 1), m, LIVE)
                .is_empty()
        );
        assert!(state.route(motion(2, 1), m, LIVE).is_empty());
    }

    /// Asserts that a cancel releases every forwarded button,
    /// lowest-numbered first, at the last cell the terminal saw.
    ///
    /// Case: the user holds both buttons over nvim and switches windows
    /// with the keyboard.
    #[test]
    fn a_cancel_releases_every_forwarded_button_in_order() {
        let mut state = PointerState::default();
        let m = modes(MouseTracking::Drag);
        state.route(press(PointerButton::Right, 2, 2), m, LIVE);
        state.route(press(PointerButton::Left, 2, 2), m, LIVE);
        state.route(motion(5, 3), m, LIVE);
        assert_eq!(
            state.route(cancel(), m, LIVE),
            vec![
                report(MouseButton::Left, MouseReportKind::Release, 5, 3),
                report(MouseButton::Right, MouseReportKind::Release, 5, 3),
            ]
        );
        assert!(
            state
                .route(release(PointerButton::Left, 5, 3), m, LIVE)
                .is_empty()
        );
    }

    /// Asserts that a cancel ends a selection drag without copying it.
    ///
    /// Case: the user is dragging a selection at a shell prompt when the
    /// window loses focus.
    #[test]
    fn a_cancel_ends_a_selection_drag_without_copying() {
        let mut state = PointerState::default();
        let off = modes(MouseTracking::Off);
        state.route(press(PointerButton::Left, 1, 1), off, LIVE);
        state.route(motion(3, 1), off, LIVE);
        assert!(state.route(cancel(), off, LIVE).is_empty());
        assert!(
            state
                .route(release(PointerButton::Left, 3, 1), off, LIVE)
                .is_empty()
        );
    }

    /// Asserts that one motion both reports the forwarded button and
    /// extends the selection a Shift-held left button began.
    ///
    /// Case: the user holds the right button over nvim, then Shift-drags
    /// with the left button to select text at the same time.
    #[test]
    fn one_motion_serves_a_forwarded_button_and_a_selection_drag() {
        let mut state = PointerState::default();
        let m = modes(MouseTracking::Drag);
        state.route(press(PointerButton::Right, 1, 1), m, LIVE);
        assert_eq!(
            state.route(with_shift(press(PointerButton::Left, 1, 1)), m, LIVE),
            vec![start(1, 1, SelectionKind::Simple)]
        );
        assert_eq!(
            state.route(motion(3, 1), m, LIVE),
            vec![
                report(MouseButton::Right, MouseReportKind::Motion, 3, 1),
                extend(3, 1),
            ]
        );
    }

    /// Asserts that a forwarded press of another button leaves a held
    /// left-button selection drag alone, so the drag keeps extending and
    /// copies on release.
    ///
    /// Case: the user Shift-drags a selection over nvim, lets go of Shift,
    /// and presses the right button before releasing the left one.
    #[test]
    fn a_forwarded_press_leaves_a_held_selection_drag_alone() {
        let mut state = PointerState::default();
        let m = modes(MouseTracking::Drag);
        state.route(with_shift(press(PointerButton::Left, 1, 1)), m, LIVE);
        state.route(motion(3, 1), m, LIVE);
        assert_eq!(
            state.route(press(PointerButton::Right, 3, 1), m, LIVE),
            vec![report(MouseButton::Right, MouseReportKind::Press, 3, 1)]
        );
        assert_eq!(
            state.route(motion(4, 1), m, LIVE),
            vec![
                report(MouseButton::Right, MouseReportKind::Motion, 4, 1),
                extend(4, 1),
            ]
        );
        assert_eq!(
            state.route(release(PointerButton::Left, 4, 1), m, LIVE),
            vec![extend(4, 1), PointerAction::Copy]
        );
    }

    /// Asserts that motion with several forwarded buttons held reports the
    /// lowest-numbered one.
    ///
    /// Case: the user holds the middle button over nvim and then also
    /// presses the left one while dragging.
    #[test]
    fn motion_reports_the_lowest_forwarded_button() {
        let mut state = PointerState::default();
        let m = modes(MouseTracking::Drag);
        state.route(press(PointerButton::Middle, 1, 1), m, LIVE);
        state.route(press(PointerButton::Left, 1, 1), m, LIVE);
        assert_eq!(
            state.route(motion(2, 1), m, LIVE),
            vec![report(MouseButton::Left, MouseReportKind::Motion, 2, 1)]
        );
    }

    /// Asserts that motion is reported only when the pointer leaves the
    /// cell of the last report of any kind.
    ///
    /// Case: the user clicks in nvim with `mousemoveevent` set, nudges the
    /// pointer inside the same cell, then moves it on.
    #[test]
    fn motion_is_deduplicated_against_the_last_reported_cell() {
        let mut state = PointerState::default();
        let m = modes(MouseTracking::Motion);
        state.route(press(PointerButton::Left, 3, 3), m, LIVE);
        assert!(state.route(motion(3, 3), m, LIVE).is_empty());
        state.route(release(PointerButton::Left, 3, 3), m, LIVE);
        assert!(state.route(motion(3, 3), m, LIVE).is_empty());
        assert_eq!(
            state.route(motion(4, 3), m, LIVE),
            vec![report(MouseButton::None, MouseReportKind::Motion, 4, 3)]
        );
    }

    /// Asserts that an event routed while tracking is off forgets the cell
    /// of the last report, so the next application's first motion into
    /// that cell is reported.
    ///
    /// Case: the user hovers over nvim with `mousemoveevent` set, quits
    /// it, moves around the shell prompt, relaunches nvim, and moves back
    /// onto the cell of the last hover report.
    #[test]
    fn tracking_off_forgets_the_last_reported_cell() {
        let mut state = PointerState::default();
        let on = modes(MouseTracking::Motion);
        state.route(motion(10, 5), on, LIVE);
        state.route(motion(9, 5), modes(MouseTracking::Off), LIVE);
        assert_eq!(
            state.route(motion(10, 5), on, LIVE),
            vec![report(MouseButton::None, MouseReportKind::Motion, 10, 5)]
        );
    }

    /// Asserts that buttonless motion is reported with Shift held, and
    /// that a scrolled-back viewport reports none.
    ///
    /// Case: the user hovers over an any-event-tracking TUI while holding
    /// Shift, then scrolls the pane back into history and hovers again.
    #[test]
    fn buttonless_motion_ignores_shift_but_needs_the_live_tail() {
        let mut state = PointerState::default();
        let m = modes(MouseTracking::Motion);
        assert_eq!(
            state.route(with_shift(motion(2, 2)), m, LIVE),
            vec![PointerAction::Report(MouseReport {
                button: MouseButton::None,
                kind: MouseReportKind::Motion,
                cell: at(2, 2),
                mods: ProtocolModifiers {
                    shift: true,
                    ..ProtocolModifiers::default()
                },
            })]
        );
        assert!(state.route(motion(3, 2), m, false).is_empty());
    }

    /// Asserts that local presses after a forwarded one count their clicks
    /// from it, whatever the host counted: the first is a single click,
    /// which copies nothing, and the next a double click, which copies.
    ///
    /// Case: the user clicks in nvim and, a moment later, Shift-double-clicks
    /// the same word.
    #[test]
    fn local_presses_after_a_forwarded_one_count_their_own_clicks() {
        let mut state = PointerState::default();
        let m = modes(MouseTracking::Drag);
        state.route(press(PointerButton::Left, 2, 2), m, LIVE);
        state.route(release(PointerButton::Left, 2, 2), m, LIVE);
        for (host_clicks, copies) in [(2, false), (3, true)] {
            let local = with_shift(with_clicks(press(PointerButton::Left, 2, 2), host_clicks));
            assert_eq!(
                state.route(local, m, LIVE),
                vec![start(2, 2, SelectionKind::Simple)],
                "host count {host_clicks}"
            );
            let released = state.route(release(PointerButton::Left, 2, 2), m, LIVE);
            assert_eq!(
                released.contains(&PointerAction::Copy),
                copies,
                "host count {host_clicks}: {released:?}"
            );
        }
    }

    /// Asserts that a second press of a still-forwarded button reports its
    /// release before routing the new press.
    ///
    /// Case: the platform drops a release while the user clicks rapidly in
    /// nvim, so the next press arrives with the button still held.
    #[test]
    fn a_repress_releases_the_stale_forwarded_button_first() {
        let mut state = PointerState::default();
        let m = modes(MouseTracking::Clicks);
        state.route(press(PointerButton::Left, 2, 2), m, LIVE);
        assert_eq!(
            state.route(press(PointerButton::Left, 4, 2), m, LIVE),
            vec![
                report(MouseButton::Left, MouseReportKind::Release, 4, 2),
                PointerAction::SelectionClear,
                report(MouseButton::Left, MouseReportKind::Press, 4, 2),
            ]
        );
    }

    /// Asserts that a single click anchors an empty selection at the press
    /// and copies nothing on a bare release, while a drag extends that
    /// selection from its first cell change on and copies on release.
    ///
    /// Case: the user clicks once at a shell prompt, then presses again
    /// and drags across four cells before letting go.
    #[test]
    fn a_single_click_arms_and_a_drag_selects_and_copies() {
        let mut state = PointerState::default();
        let off = modes(MouseTracking::Off);
        assert_eq!(
            state.route(press(PointerButton::Left, 5, 5), off, LIVE),
            vec![start(5, 5, SelectionKind::Simple)]
        );
        assert!(
            state
                .route(release(PointerButton::Left, 5, 5), off, LIVE)
                .is_empty()
        );
        state.route(press(PointerButton::Left, 5, 5), off, LIVE);
        assert!(state.route(motion(5, 5), off, LIVE).is_empty());
        assert_eq!(state.route(motion(7, 5), off, LIVE), vec![extend(7, 5)]);
        assert_eq!(state.route(motion(9, 5), off, LIVE), vec![extend(9, 5)]);
        assert_eq!(
            state.route(release(PointerButton::Left, 9, 5), off, LIVE),
            vec![extend(9, 5), PointerAction::Copy]
        );
    }

    /// Asserts that releasing a selection drag moves the selection's end
    /// to the half of the cell it was released in before copying.
    ///
    /// Case: the user drags right across a word at a shell prompt, entering
    /// its last letter on the left half, and lets go on the right half.
    #[test]
    fn a_drag_release_extends_to_the_released_half_before_copying() {
        let mut state = PointerState::default();
        let off = modes(MouseTracking::Off);
        state.route(press(PointerButton::Left, 1, 1), off, LIVE);
        state.route(motion(5, 1), off, LIVE);
        let mut released = release(PointerButton::Left, 5, 1);
        released.side = CellSide::Right;
        assert_eq!(
            state.route(released, off, LIVE),
            vec![
                PointerAction::SelectionExtend {
                    cell: at(5, 1),
                    side: CellSide::Right
                },
                PointerAction::Copy,
            ]
        );
    }

    /// Asserts that a drag's end is known only once the drag has left the
    /// cell it was pressed on, follows the cells it reaches, and is gone
    /// after the release.
    ///
    /// Case: the user presses at a shell prompt and drags down a row while
    /// spinning the wheel, then lets go.
    #[test]
    fn the_drag_end_is_known_only_while_a_drag_is_under_way() {
        let mut state = PointerState::default();
        let off = modes(MouseTracking::Off);
        state.route(press(PointerButton::Left, 2, 2), off, LIVE);
        assert_eq!(state.drag_end(), None);
        state.route(motion(4, 3), off, LIVE);
        assert_eq!(state.drag_end(), Some((at(4, 3), CellSide::Left)));
        state.route(release(PointerButton::Left, 4, 3), off, LIVE);
        assert_eq!(state.drag_end(), None);
    }

    /// Asserts that a double click selects plainly at once, a triple click
    /// or an Alt click selects lines at once, each copying on release, and
    /// that a zero click count is a single click.
    ///
    /// Case: the user double-clicks a word, triple-clicks a line, and
    /// Alt-clicks another line at a shell prompt.
    #[test]
    fn multi_clicks_and_alt_select_at_once() {
        let off = modes(MouseTracking::Off);
        for (input, kind) in [
            (
                with_clicks(press(PointerButton::Left, 2, 2), 2),
                SelectionKind::Simple,
            ),
            (
                with_clicks(press(PointerButton::Left, 2, 2), 3),
                SelectionKind::Lines,
            ),
            (
                with_clicks(press(PointerButton::Left, 2, 2), 7),
                SelectionKind::Lines,
            ),
            (
                with_alt(press(PointerButton::Left, 2, 2)),
                SelectionKind::Lines,
            ),
        ] {
            let mut state = PointerState::default();
            assert_eq!(state.route(input, off, LIVE), vec![start(2, 2, kind)]);
            assert_eq!(
                state.route(release(PointerButton::Left, 2, 2), off, LIVE),
                vec![extend(2, 2), PointerAction::Copy]
            );
        }
        let mut state = PointerState::default();
        state.route(with_clicks(press(PointerButton::Left, 2, 2), 0), off, LIVE);
        assert!(
            state
                .route(release(PointerButton::Left, 2, 2), off, LIVE)
                .is_empty()
        );
    }

    /// Asserts that a middle or right button that is not forwarded does
    /// nothing, from press to release.
    ///
    /// Case: the user right-clicks and middle-clicks at a plain shell
    /// prompt.
    #[test]
    fn unforwarded_middle_and_right_buttons_do_nothing() {
        let off = modes(MouseTracking::Off);
        for button in [PointerButton::Middle, PointerButton::Right] {
            let mut state = PointerState::default();
            assert!(state.route(press(button, 2, 2), off, LIVE).is_empty());
            assert!(state.route(motion(4, 2), off, LIVE).is_empty());
            assert!(state.route(release(button, 4, 2), off, LIVE).is_empty());
        }
    }

    /// Asserts that no routed event yields both a report and a copy.
    ///
    /// Case: the user mixes clicks, Shift-drags, chords, and a focus loss
    /// over a pane whose application keeps toggling tracking.
    #[test]
    fn no_event_yields_both_a_report_and_a_copy() {
        let script = [
            press(PointerButton::Left, 1, 1),
            motion(2, 1),
            with_shift(press(PointerButton::Left, 2, 1)),
            motion(4, 1),
            press(PointerButton::Right, 4, 1),
            motion(5, 2),
            release(PointerButton::Left, 5, 2),
            release(PointerButton::Right, 5, 2),
            with_shift(press(PointerButton::Left, 1, 1)),
            motion(3, 3),
            cancel(),
        ];
        for tracking in [
            MouseTracking::Off,
            MouseTracking::Clicks,
            MouseTracking::Drag,
            MouseTracking::Motion,
        ] {
            let mut state = PointerState::default();
            for (step, input) in script.into_iter().enumerate() {
                let m = modes(if step % 2 == 0 {
                    tracking
                } else {
                    MouseTracking::Off
                });
                let actions = state.route(input, m, LIVE);
                let copies = actions.contains(&PointerAction::Copy);
                assert!(
                    !(copies && reports(&actions) > 0),
                    "{tracking:?} step {step}: {actions:?}"
                );
            }
        }
    }
}
