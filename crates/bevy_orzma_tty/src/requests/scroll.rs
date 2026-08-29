//! `RequestTtyScroll`: the viewport movement the host UI asks a terminal
//! entity to perform.

use bevy::prelude::*;
use orzma_vt::prelude::Scroll;

use crate::OrzmaTtyHandle;

/// Fired by the host UI to move a specific terminal entity's viewport.
///
/// The motion vocabulary is [`Scroll`] itself — the request carries
/// exactly what the VT applies, and the observer's only job is routing
/// it to the targeted entity's handle. Clamping at both ends of history
/// and page-size resolution live in `orzma_vt` and are pinned by its
/// tests, not re-asserted here.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyScroll {
    #[event_target]
    pub terminal: Entity,
    /// The movement to perform.
    pub scroll: Scroll,
}

pub(super) struct ScrollPlugin;

impl Plugin for ScrollPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_scroll);
    }
}

fn apply_scroll(e: On<RequestTtyScroll>, mut terms: Query<&mut OrzmaTtyHandle>) {
    if let Ok(mut tty) = terms.get_mut(e.terminal) {
        tty.scroll(e.scroll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OrzmaTtyHandle;
    use orzma_vt::prelude::{DisplayOffset, Vt};

    // NOTE: on the 24-row grid the first 23 newlines only fill the
    // viewport (a row is pushed into history only once the cursor
    // already sits on the last screen line), so `history_rows + 23`
    // lines seed exactly `history_rows` — getting this wrong shifts
    // every `display_offset` expectation below.
    fn app_with_terminal(history_rows: usize) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(ScrollPlugin);
        let (mut handle, _) = OrzmaTtyHandle::detached(80, 24);
        let seed: Vec<u8> = (0..history_rows + 23)
            .flat_map(|i| format!("l{i}\r\n").into_bytes())
            .collect();
        handle.feed_bytes(&seed);
        let terminal = app.world_mut().spawn(handle).id();
        (app, terminal)
    }

    fn trigger_scroll(app: &mut App, terminal: Entity, scroll: Scroll) {
        app.world_mut()
            .trigger(RequestTtyScroll { terminal, scroll });
    }

    fn display_offset(app: &App, terminal: Entity) -> DisplayOffset {
        app.world()
            .get::<OrzmaTtyHandle>(terminal)
            .expect("terminal entity must keep its handle")
            .vt()
            .display_offset()
    }

    /// Asserts that positive and negative `Delta` requests move the
    /// viewport in opposite directions, cumulatively.
    ///
    /// Case: the user scrolls with the mouse wheel, where each notch
    /// fires one request and a fast spin delivers a burst of them.
    #[test]
    #[ignore = "DeviceState::scroll is still todo!(), so a scroll request panics"]
    fn scroll_up_and_down_move_the_viewport_relatively() {
        let (mut app, terminal) = app_with_terminal(10);
        trigger_scroll(&mut app, terminal, Scroll::Delta(3));
        assert_eq!(display_offset(&app, terminal), DisplayOffset(3));
        trigger_scroll(&mut app, terminal, Scroll::Delta(-2));
        assert_eq!(display_offset(&app, terminal), DisplayOffset(1));
    }

    /// Asserts that `Top` lands on the oldest retained line and
    /// `Bottom` returns to the live tail, regardless of the current
    /// offset.
    ///
    /// Case: the user jumps to the oldest history and back to the
    /// live tail with the vi-mode `gg` and `G` motions.
    #[test]
    #[ignore = "DeviceState::scroll is still todo!(), so a scroll request panics"]
    fn scroll_top_and_bottom_jump_to_the_extremes() {
        let (mut app, terminal) = app_with_terminal(10);
        trigger_scroll(&mut app, terminal, Scroll::Top);
        assert_eq!(display_offset(&app, terminal), DisplayOffset(10));
        trigger_scroll(&mut app, terminal, Scroll::Bottom);
        assert_eq!(display_offset(&app, terminal), DisplayOffset(0));
    }

    /// Asserts the agreed page semantics: one page is the full grid
    /// height (24 rows here) and half a page is half that.
    ///
    /// Case: the user pages through history with the vi-mode
    /// `Ctrl-B`/`Ctrl-F`/`Ctrl-U`/`Ctrl-D` motions or
    /// `Shift+PageUp`/`PageDown`. The decided policy is that one page
    /// is the full screen height (xterm-style, with no overlap line)
    /// rather than `rows - 1`.
    #[test]
    #[ignore = "DeviceState::scroll is still todo!(), so a scroll request panics"]
    fn paged_scrolls_move_by_screenfuls() {
        let (mut app, terminal) = app_with_terminal(40);
        trigger_scroll(&mut app, terminal, Scroll::PageUp);
        assert_eq!(display_offset(&app, terminal), DisplayOffset(24));
        trigger_scroll(&mut app, terminal, Scroll::HalfPageUp);
        assert_eq!(display_offset(&app, terminal), DisplayOffset(36));
        trigger_scroll(&mut app, terminal, Scroll::HalfPageDown);
        assert_eq!(display_offset(&app, terminal), DisplayOffset(24));
        trigger_scroll(&mut app, terminal, Scroll::PageDown);
        assert_eq!(display_offset(&app, terminal), DisplayOffset(0));
    }
}
