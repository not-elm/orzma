//! The vi-cursor motion the host UI asks a terminal entity to perform.

use crate::OrzmuxConnection;
use crate::requests::PaneSender;
use bevy::prelude::*;
pub use orzma_vt::prelude::ViMotion;
use orzmux::prelude::OrzmuxCommand;

/// Fired by the host UI to move a specific terminal entity's vi cursor.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyViMotion {
    #[event_target]
    pub terminal: Entity,
    /// The motion to apply to the vi cursor.
    pub motion: ViMotion,
}

pub(super) struct ViMotionPlugin;

impl Plugin for ViMotionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_vi_motion.run_if(resource_exists::<OrzmuxConnection>));
    }
}

fn apply_vi_motion(e: On<RequestTtyViMotion>, panes: PaneSender) {
    panes.send_for(e.terminal, |pane| OrzmuxCommand::ViMotion {
        pane,
        motion: e.motion,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent, spawn_pane};
    use orzmux::prelude::{OrzmuxCommand, PaneId};

    /// Every `(target, motion)` an observer saw, in fire order.
    #[derive(Resource, Default)]
    struct Seen(Vec<(Entity, ViMotion)>);

    /// Observer that appends what it received to [`Seen`].
    fn record(ev: On<RequestTtyViMotion>, mut seen: ResMut<Seen>) {
        seen.0.push((ev.event_target(), ev.motion));
    }

    /// The full motion vocabulary, in declaration order.
    const ALL: [ViMotion; 21] = [
        ViMotion::Up,
        ViMotion::Down,
        ViMotion::Left,
        ViMotion::Right,
        ViMotion::First,
        ViMotion::Last,
        ViMotion::FirstOccupied,
        ViMotion::High,
        ViMotion::Middle,
        ViMotion::Low,
        ViMotion::SemanticLeft,
        ViMotion::SemanticRight,
        ViMotion::SemanticLeftEnd,
        ViMotion::SemanticRightEnd,
        ViMotion::WordLeft,
        ViMotion::WordRight,
        ViMotion::WordLeftEnd,
        ViMotion::WordRightEnd,
        ViMotion::Bracket,
        ViMotion::ParagraphUp,
        ViMotion::ParagraphDown,
    ];

    /// Asserts that a triggered `RequestTtyViMotion` reaches an observer with
    /// its target and motion intact.
    ///
    /// Case: one keymapped motion (`j`) resolved by the host and fired at the
    /// terminal in vi mode.
    #[test]
    fn trigger_delivers_the_requested_motion() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(RequestTtyViMotion {
            terminal,
            motion: ViMotion::Down,
        });

        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![(terminal, ViMotion::Down)]
        );
    }

    /// Asserts that every motion survives the trigger unchanged, in order.
    ///
    /// Case: a held key repeating, and the near-miss pairs the
    /// vocabulary is full of — `SemanticLeft` vs. `SemanticLeftEnd`,
    /// `WordRight` vs. `WordRightEnd`.
    #[test]
    fn every_motion_round_trips_in_order() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();

        for motion in ALL {
            app.world_mut()
                .trigger(RequestTtyViMotion { terminal, motion });
        }

        let seen: Vec<ViMotion> = app
            .world()
            .resource::<Seen>()
            .0
            .iter()
            .map(|(_, motion)| *motion)
            .collect();
        assert_eq!(seen, ALL);
    }

    /// Asserts that no two motions compare equal.
    ///
    /// Case: a motion is added to the enum by copying a neighbouring
    /// variant.
    #[test]
    fn all_motions_are_distinct() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a, b, "{a:?} and {b:?} must be distinct motions");
            }
        }
    }

    /// Asserts that a motion request becomes a `ViMotion` command for the
    /// addressed pane.
    ///
    /// Case: the user presses `j` in vi mode on a pane.
    #[test]
    fn a_motion_request_becomes_a_vi_motion_command_for_the_pane() {
        let (mut app, commands) = app_with_connection(ViMotionPlugin);
        let pane = spawn_pane(&mut app, PaneId(5));
        app.world_mut().trigger(RequestTtyViMotion {
            terminal: pane,
            motion: ViMotion::Down,
        });
        let sent = sent(&commands);
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            sent[0],
            OrzmuxCommand::ViMotion {
                pane: PaneId(5),
                motion: ViMotion::Down
            }
        ));
    }
}
