//! `RequestTermViMotion`: the vi-cursor motion the host UI asks a terminal
//! entity to perform.
//!
//! [`ViMotion`] mirrors the motion vocabulary one-for-one. Its final home is
//! the VT layer; it is defined here until that crate owns the type, at which
//! point this becomes a re-export.

use bevy::prelude::*;

/// Fired by the host UI to move a specific terminal entity's vi cursor.
///
/// Has no effect outside vi mode; the apply observer holds that state, so the
/// host may fire without checking first.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermViMotion {
    #[event_target]
    pub terminal: Entity,
    /// The motion to apply to the vi cursor.
    pub motion: ViMotion,
}

/// A vi-cursor motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViMotion {
    /// One line up.
    Up,
    /// One line down.
    Down,
    /// One cell left.
    Left,
    /// One cell right.
    Right,
    /// First column of the line.
    First,
    /// Last column of the line.
    Last,
    /// First non-blank column of the line.
    FirstOccupied,
    /// Top line of the viewport.
    High,
    /// Middle line of the viewport.
    Middle,
    /// Bottom line of the viewport.
    Low,
    /// Start of the previous semantic word.
    SemanticLeft,
    /// Start of the next semantic word.
    SemanticRight,
    /// End of the previous semantic word.
    SemanticLeftEnd,
    /// End of the next semantic word.
    SemanticRightEnd,
    /// Start of the previous whitespace-delimited word.
    WordLeft,
    /// Start of the next whitespace-delimited word.
    WordRight,
    /// End of the previous whitespace-delimited word.
    WordLeftEnd,
    /// End of the next whitespace-delimited word.
    WordRightEnd,
    /// Matching bracket of the one under the cursor.
    Bracket,
    /// Previous paragraph break.
    ParagraphUp,
    /// Next paragraph break.
    ParagraphDown,
}

pub(super) struct ViMotionPlugin;

impl Plugin for ViMotionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_vi_motion);
    }
}

fn apply_vi_motion(_e: On<RequestTermViMotion>) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `(target, motion)` an observer saw, in fire order.
    #[derive(Resource, Default)]
    struct Seen(Vec<(Entity, ViMotion)>);

    /// Observer that appends what it received to [`Seen`].
    fn record(ev: On<RequestTermViMotion>, mut seen: ResMut<Seen>) {
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

    /// Asserts that a triggered `RequestTermViMotion` reaches an observer with
    /// its target and motion intact.
    ///
    /// Case: one keymapped motion (`j`) resolved by the host and fired at the
    /// terminal in vi mode.
    #[test]
    fn trigger_delivers_the_requested_motion() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(RequestTermViMotion {
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
    /// Case: a held key repeating, and the near-miss pairs the vocabulary is
    /// full of — `SemanticLeft` vs. `SemanticLeftEnd`, `WordRight` vs.
    /// `WordRightEnd`. Those differ by one cell in the resulting cursor
    /// position, so a mis-ordered or collapsed variant is invisible in a
    /// single-motion test but wrong on screen.
    #[test]
    fn every_motion_round_trips_in_order() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();

        for motion in ALL {
            app.world_mut()
                .trigger(RequestTermViMotion { terminal, motion });
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
    /// Case: the guard for the `ALL` fixture above and for the enum itself. A
    /// duplicated variant (easy to introduce when adding a motion by copying a
    /// neighbouring line) would let `every_motion_round_trips_in_order` pass
    /// while two distinct keybindings silently share one behaviour.
    #[test]
    fn all_motions_are_distinct() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a, b, "{a:?} and {b:?} must be distinct motions");
            }
        }
    }
}
