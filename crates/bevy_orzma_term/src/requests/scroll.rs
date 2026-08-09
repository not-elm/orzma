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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `(target, kind)` an observer saw, in fire order.
    #[derive(Resource, Default)]
    struct Seen(Vec<(Entity, ScrollKind)>);

    /// Observer that appends what it received to [`Seen`].
    fn record(ev: On<RequestTermScroll>, mut seen: ResMut<Seen>) {
        seen.0.push((ev.event_target(), ev.kind));
    }

    /// Asserts that a triggered `RequestTermScroll` reaches an observer with
    /// its target and movement intact.
    ///
    /// Case: the wheel path — a notch resolves to a line count and fires one
    /// request at the hovered terminal.
    #[test]
    fn trigger_delivers_the_requested_movement() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(RequestTermScroll {
            terminal,
            kind: ScrollKind::Up(3),
        });

        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![(terminal, ScrollKind::Up(3))]
        );
    }

    /// Asserts that every `ScrollKind` variant survives the trigger unchanged.
    ///
    /// Case: the vi keymap emits the whole vocabulary (`ViModeScroll` maps onto
    /// all eight variants). A variant that normalizes on the way — say a future
    /// `Up(0)` folded into `Bottom`, or `PageUp` rewritten as `Up(rows)` — would
    /// silently change what the apply observer receives, and the row count it
    /// would need for that rewrite is not available at trigger time anyway.
    #[test]
    fn every_variant_round_trips() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();
        let all = [
            ScrollKind::Up(1),
            ScrollKind::Down(1),
            ScrollKind::Up(0),
            ScrollKind::PageUp,
            ScrollKind::PageDown,
            ScrollKind::HalfPageUp,
            ScrollKind::HalfPageDown,
            ScrollKind::Top,
            ScrollKind::Bottom,
        ];

        for kind in all {
            app.world_mut()
                .trigger(RequestTermScroll { terminal, kind });
        }

        let seen: Vec<ScrollKind> = app
            .world()
            .resource::<Seen>()
            .0
            .iter()
            .map(|(_, kind)| *kind)
            .collect();
        assert_eq!(seen, all);
    }

    /// Asserts that opposite directions stay distinguishable.
    ///
    /// Case: the guard against collapsing `Up`/`Down` back into one signed
    /// field. `Up(3)` and `Down(3)` are the same magnitude, so an accidental
    /// `unsigned_abs`-style normalization in a later refactor would make the
    /// viewport scroll the wrong way with no compile error.
    #[test]
    fn up_and_down_of_equal_magnitude_are_not_equal() {
        assert_ne!(ScrollKind::Up(3), ScrollKind::Down(3));
        assert_ne!(ScrollKind::PageUp, ScrollKind::PageDown);
        assert_ne!(ScrollKind::Top, ScrollKind::Bottom);
    }
}
