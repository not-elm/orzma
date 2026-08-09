//! `RequestTermScroll`: the viewport movement the host UI asks a terminal
//! entity to perform.

use bevy::prelude::*;

/// Fired by the host UI to move a specific terminal entity's viewport.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermScroll {
    #[event_target]
    pub terminal: Entity,
    /// The movement to perform.
    pub kind: ScrollKind,
}

/// A viewport movement, named by direction rather than by a signed delta.
///
/// Scrollback grows upward from the live tail, so a signed line count has two
/// equally plausible readings ("positive is toward history" vs. "positive is
/// toward the tail") and callers on the wheel path and the vi path disagree
/// about which. Naming the direction removes the ambiguity from the request
/// itself: the apply observer is the single place that converts to whatever
/// sign the VT expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollKind {
    /// Move `lines` toward older output (deeper into scrollback).
    Up(u32),
    /// Move `lines` toward the live tail.
    Down(u32),
    /// One screenful toward older output.
    PageUp,
    /// One screenful toward the live tail.
    PageDown,
    /// Half a screenful toward older output.
    HalfPageUp,
    /// Half a screenful toward the live tail.
    HalfPageDown,
    /// The oldest line still in scrollback.
    Top,
    /// The live tail.
    Bottom,
}

pub(super) struct ScrollPlugin;

impl Plugin for ScrollPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_scroll);
    }
}

fn apply_scroll(e: On<RequestTermScroll>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OrzmaTermHandle;
    use orzma_vt::prelude::OrzmaVt;

    // NOTE: on the 24-row grid the first 23 newlines only fill the
    // viewport (alacritty pushes a row into history once the cursor
    // already sits on the last screen line), so `history_rows + 23`
    // lines seed exactly `history_rows` — getting this wrong shifts
    // every `display_offset` expectation below.
    fn app_with_terminal(history_rows: usize) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(ScrollPlugin);
        let (mut handle, _) = OrzmaTermHandle::detached(80, 24);
        let seed: Vec<u8> = (0..history_rows + 23)
            .flat_map(|i| format!("l{i}\r\n").into_bytes())
            .collect();
        handle.vt_mut().interpret(&seed);
        let terminal = app.world_mut().spawn(handle).id();
        (app, terminal)
    }

    fn trigger_scroll(app: &mut App, terminal: Entity, kind: ScrollKind) {
        app.world_mut()
            .trigger(RequestTermScroll { terminal, kind });
    }

    fn display_offset(app: &mut App, terminal: Entity) -> u32 {
        app.world_mut()
            .get_mut::<OrzmaTermHandle>(terminal)
            .expect("terminal entity must keep its handle")
            .vt_mut()
            .display_offset()
    }

    /// Asserts that `Up`/`Down` move the viewport by the requested line
    /// count in opposite directions, cumulatively.
    ///
    /// Case: the wheel path — each notch resolves to a line count and
    /// fires one request, and bursts of notches must accumulate. The
    /// apply observer is the single place that converts the named
    /// direction into the VT's signed delta (the reason `ScrollKind`
    /// has no signed field), so a sign slip there scrolls the viewport
    /// the wrong way with no compile error; the `Up`-then-`Down`
    /// sequence pins both mappings against each other.
    #[test]
    fn scroll_up_and_down_move_the_viewport_relatively() {
        let (mut app, terminal) = app_with_terminal(10);
        trigger_scroll(&mut app, terminal, ScrollKind::Up(3));
        assert_eq!(display_offset(&mut app, terminal), 3);
        trigger_scroll(&mut app, terminal, ScrollKind::Down(2));
        assert_eq!(display_offset(&mut app, terminal), 1);
    }

    /// Asserts that `Top` lands on the oldest retained line and
    /// `Bottom` returns to the live tail, regardless of the current
    /// offset.
    ///
    /// Case: the vi-mode `gg` / `G` jumps — absolute motions, unlike
    /// the wheel's relative ones. `Top` must clamp to the real history
    /// depth via a finite delta (a naive `scroll(i32::MAX)` overflows
    /// alacritty's offset arithmetic and panics under debug overflow
    /// checks), and `Bottom` is the escape hatch every scroll-back
    /// session ends with.
    #[test]
    fn scroll_top_and_bottom_jump_to_the_extremes() {
        let (mut app, terminal) = app_with_terminal(10);
        trigger_scroll(&mut app, terminal, ScrollKind::Top);
        assert_eq!(display_offset(&mut app, terminal), 10);
        trigger_scroll(&mut app, terminal, ScrollKind::Bottom);
        assert_eq!(display_offset(&mut app, terminal), 0);
    }

    /// Asserts the agreed page semantics: one page is the full grid
    /// height (24 rows here) and half a page is half that.
    ///
    /// Case: the vi-mode `Ctrl-B`/`Ctrl-F`/`Ctrl-U`/`Ctrl-D` and
    /// `Shift+PageUp`/`PageDown` paths. The decided policy matches
    /// xterm / alacritty `Scroll::PageUp` (full screen, no overlap
    /// line) — do not "fix" this test toward `rows - 1`. The page size
    /// must come from the live grid height, which only the apply layer
    /// can see; clamping at the ends of history is the VT's job and is
    /// pinned by `orzma_vt`'s tests, so it is not re-asserted here.
    #[test]
    fn paged_scrolls_move_by_screenfuls() {
        let (mut app, terminal) = app_with_terminal(40);
        trigger_scroll(&mut app, terminal, ScrollKind::PageUp);
        assert_eq!(display_offset(&mut app, terminal), 24);
        trigger_scroll(&mut app, terminal, ScrollKind::HalfPageUp);
        assert_eq!(display_offset(&mut app, terminal), 36);
        trigger_scroll(&mut app, terminal, ScrollKind::HalfPageDown);
        assert_eq!(display_offset(&mut app, terminal), 24);
        trigger_scroll(&mut app, terminal, ScrollKind::PageDown);
        assert_eq!(display_offset(&mut app, terminal), 0);
    }
}
