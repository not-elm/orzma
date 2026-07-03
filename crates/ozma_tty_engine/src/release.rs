//! Control-mode release: returns an adopted gateway terminal to normal VT
//! feeding when the tmux control stream ends (a detach).

use crate::coalescer::Coalescer;
use crate::control_mode::{AdoptedControlMode, ControlModeDetected, ControlModeWatch, Handover};
use crate::handle::TerminalHandle;
use crate::ingest_and_flush_or_arm;
use crate::title::TerminalTitle;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::prelude::*;

/// Returns an adopted terminal to normal VT feeding.
///
/// `residual` is fed to the terminal ahead of any bytes still buffered on its
/// [`AdoptedControlMode`], both routed through the control-mode introducer
/// scanner — a fresh `tmux -CC` introducer inside the bytes re-adopts the
/// terminal (re-firing [`ControlModeDetected`]) instead of leaking protocol
/// bytes into the VT.
#[derive(EntityEvent)]
pub struct ReleaseControlMode {
    /// The adopted gateway terminal to release.
    #[event_target]
    pub entity: Entity,
    /// Terminal-bound bytes: the caller's synthesized detach line plus
    /// everything the protocol client received after the DCS terminator.
    pub residual: Vec<u8>,
}

/// Registers the release observer.
pub(crate) struct ControlModeReleasePlugin;

impl Plugin for ControlModeReleasePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_release_control_mode);
    }
}

/// Un-adopts the target terminal: feeds residual + late-captured bytes
/// through the introducer scanner into the VT (normal flush-or-arm
/// semantics), drains control events, and swaps `AdoptedControlMode` back to
/// `ControlModeWatch` — unless a fresh introducer keeps it adopted.
fn on_release_control_mode(
    ev: On<ReleaseControlMode>,
    mut commands: Commands,
    mut terminals: Query<(
        &mut TerminalHandle,
        &mut AdoptedControlMode,
        &mut Coalescer,
        &mut TerminalTitle,
    )>,
) {
    let entity = ev.entity;
    let Ok((mut handle, mut adopted, mut coalescer, mut title)) = terminals.get_mut(entity) else {
        return;
    };
    let mut bytes = ev.residual.clone();
    bytes.extend_from_slice(&adopted.take_captured());
    let mut watch = ControlModeWatch::default();
    match Handover::scan(&mut watch, &bytes) {
        Handover::NotYet { vt } => {
            ingest_and_flush_or_arm(&mut commands, entity, &mut handle, &mut coalescer, &vt);
            commands
                .entity(entity)
                .remove::<AdoptedControlMode>()
                .insert(watch);
        }
        Handover::Detected { vt, captured } => {
            ingest_and_flush_or_arm(&mut commands, entity, &mut handle, &mut coalescer, &vt);
            // NOTE: stay adopted — the released stream re-entered control mode
            // (a fresh `tmux -CC` inside the residue). Swapping to a watch here
            // would feed the new protocol stream into the VT and corrupt it;
            // keeping the capture and re-firing ControlModeDetected preserves
            // the stream for the next adoption.
            adopted.captured = captured;
            commands.trigger(ControlModeDetected { entity });
        }
    }
    handle.drain_control_events(&mut commands, entity, &mut title);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct DetectedCount(usize);

    fn count_detected(_ev: On<ControlModeDetected>, mut count: ResMut<DetectedCount>) {
        count.0 += 1;
    }

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<DetectedCount>()
            .add_observer(on_release_control_mode)
            .add_observer(count_detected);
        app
    }

    fn spawn_adopted(app: &mut App, captured: &[u8]) -> Entity {
        app.world_mut()
            .spawn((
                TerminalHandle::detached(80, 24),
                AdoptedControlMode::from_captured(captured.to_vec()),
                Coalescer::new(),
                TerminalTitle::default(),
            ))
            .id()
    }

    #[test]
    fn release_swaps_adoption_back_to_watch() {
        let mut app = build_app();
        let entity = spawn_adopted(&mut app, b"late$ ");

        app.world_mut().trigger(ReleaseControlMode {
            entity,
            residual: b"[detached (from session main)]\r\n".to_vec(),
        });
        app.update();

        let world = app.world();
        assert!(
            world.get::<AdoptedControlMode>(entity).is_none(),
            "AdoptedControlMode removed on release"
        );
        assert!(
            world.get::<ControlModeWatch>(entity).is_some(),
            "ControlModeWatch re-armed on release"
        );
        assert_eq!(
            world.resource::<DetectedCount>().0,
            0,
            "no introducer in the bytes, no re-adoption"
        );
    }

    #[test]
    fn release_with_fresh_introducer_stays_adopted_and_refires_detected() {
        let mut app = build_app();
        let entity = spawn_adopted(&mut app, b"");

        app.world_mut().trigger(ReleaseControlMode {
            entity,
            residual: b"[detached]\r\n$ tmux -CC\r\n\x1bP1000p%begin 2\r\n".to_vec(),
        });
        app.update();

        {
            let world = app.world();
            assert!(
                world.get::<AdoptedControlMode>(entity).is_some(),
                "a fresh introducer must keep the terminal adopted"
            );
            assert!(
                world.get::<ControlModeWatch>(entity).is_none(),
                "no watch while adopted"
            );
            assert_eq!(world.resource::<DetectedCount>().0, 1);
        }
        let captured = app
            .world_mut()
            .get_mut::<AdoptedControlMode>(entity)
            .unwrap()
            .take_captured();
        assert_eq!(captured, b"\x1bP1000p%begin 2\r\n".to_vec());
    }

    #[test]
    fn release_on_non_adopted_entity_is_a_noop() {
        let mut app = build_app();
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(ReleaseControlMode {
            entity,
            residual: b"x".to_vec(),
        });
        app.update();
        assert!(app.world().get::<ControlModeWatch>(entity).is_none());
    }
}
