//! `RequestTermResize`: the new grid size the host UI asks a terminal
//! entity to adopt.

use bevy::prelude::*;

/// Fired by the host UI to resize a specific terminal entity's grid.
///
/// Carries the target size in cells, not pixels — the host owns the
/// cell-metrics math and hands over an already-resolved column/row count.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermResize {
    #[event_target]
    pub terminal: Entity,
    /// Target column count.
    pub cols: u16,
    /// Target row count.
    pub rows: u16,
}

pub(super) struct ResizePlugin;

impl Plugin for ResizePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_resize);
    }
}

fn apply_resize(e: On<RequestTermResize>) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `(target, cols, rows)` an observer saw, in fire order.
    #[derive(Resource, Default)]
    struct Seen(Vec<(Entity, u16, u16)>);

    /// Observer that appends what it received to [`Seen`].
    fn record(ev: On<RequestTermResize>, mut seen: ResMut<Seen>) {
        seen.0.push((ev.event_target(), ev.cols, ev.rows));
    }

    /// Asserts that a triggered `RequestTermResize` reaches an observer with
    /// its target and both dimensions intact.
    ///
    /// Case: the ordinary window-resize path — the host has already resolved
    /// pixels to cells and fires one request at the terminal it owns. The
    /// apply observer has no other source for the new size, so anything the
    /// event drops or reorders here is lost outright.
    #[test]
    fn trigger_delivers_the_requested_size() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(RequestTermResize {
            terminal,
            cols: 120,
            rows: 40,
        });

        assert_eq!(app.world().resource::<Seen>().0, vec![(terminal, 120, 40)]);
    }

    /// Asserts that `#[event_target]` routes the event by the `terminal`
    /// field and by nothing else.
    ///
    /// Case: several terminal entities coexist (planned split panes) and each
    /// carries its own entity-scoped observer. Only the entity named by
    /// `terminal` may fire. Moving the attribute to another field, or adding
    /// a second `Entity` field ahead of it, would silently resize a different
    /// terminal — a mix-up the type checker cannot catch, since every
    /// candidate field has the same `Entity` type.
    #[test]
    fn only_the_terminal_field_receives_the_event() {
        let mut app = App::new();
        app.init_resource::<Seen>();
        let other = app.world_mut().spawn_empty().id();
        let terminal = app.world_mut().spawn_empty().id();
        for entity in [other, terminal] {
            app.world_mut().entity_mut(entity).observe(record);
        }
        app.world_mut().flush();

        app.world_mut().trigger(RequestTermResize {
            terminal,
            cols: 100,
            rows: 30,
        });

        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![(terminal, 100, 30)],
            "the observer attached to `other` must not see a resize aimed at `terminal`"
        );
    }

    /// Asserts that a zero-sized request is delivered rather than filtered.
    ///
    /// Case: a degenerate window size — a minimized window, or a frame before
    /// the cell metrics have loaded — makes the host compute `0x0`. The event
    /// is a plain request carrier, so validation and clamping belong to the
    /// apply observer; this pins that boundary so a later edit cannot quietly
    /// move the policy into the event type, where no consumer would see it.
    #[test]
    fn a_degenerate_size_is_delivered_unchanged() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(RequestTermResize {
            terminal,
            cols: 0,
            rows: 0,
        });

        assert_eq!(app.world().resource::<Seen>().0, vec![(terminal, 0, 0)]);
    }
}
