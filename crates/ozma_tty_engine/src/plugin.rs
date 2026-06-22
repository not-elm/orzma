//! `TerminalHandlePlugin` — registers the 4 chained Bevy systems.

use crate::coalescer::Coalescer;
use crate::control_mode::{
    AdoptedControlMode, ControlModeDetected, ControlModeWatch, Handover, scan_handover,
};
use crate::events::{TerminalChildExit, TerminalKeyInput};
use crate::handle::TerminalHandle;
use crate::input_codec::encode_key;
use crate::pty::PtyHandle;
use crate::raw_write::RawWritePlugin;
use crate::title::TerminalTitle;
use bevy::ecs::entity::Entity;
use bevy::ecs::observer::On;
use bevy::ecs::system::ParallelCommands;
use bevy::prelude::*;
use std::time::Instant;

/// Adds the four-system terminal bridge to the Bevy app's `Update`
/// schedule.
pub struct TerminalHandlePlugin;

impl Plugin for TerminalHandlePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(RawWritePlugin)
            .add_systems(
                Update,
                (
                    drain_pty_chunks,
                    drain_pty_writes,
                    check_deadline_flush,
                    drain_pty_exits,
                )
                    .chain(),
            )
            .add_observer(on_terminal_key_input);
    }
}

/// Drains PTY output into alacritty `Term`, classifies damage, and
/// either immediately flushes or arms the coalescer.
///
/// Also drains control events (Bell/Title/ResetTitle/Clipboard)
/// produced by the listener while parsing the chunks.
fn drain_pty_chunks(
    par_commands: ParallelCommands,
    mut terminals: Query<(
        Entity,
        &mut TerminalHandle,
        &mut PtyHandle,
        &mut Coalescer,
        &mut TerminalTitle,
        Option<&mut ControlModeWatch>,
        Option<&mut AdoptedControlMode>,
    )>,
) {
    terminals.par_iter_mut().for_each(
        |(entity, mut handle, mut pty, mut coalescer, mut title, watch, adopted)| {
            process_pty_chunks(
                &par_commands,
                entity,
                &mut handle,
                &mut pty,
                &mut coalescer,
                watch,
                adopted,
            );
            process_control_events(&par_commands, entity, &handle, &mut title);
        },
    );
}

/// Drains `reply_rx` (alacritty PtyWrite responses) and writes them
/// back to the PTY. Concatenates per-entity into one `write_all` to
/// minimize syscalls.
fn drain_pty_writes(mut q: Query<(&TerminalHandle, &mut PtyHandle)>) {
    q.par_iter_mut().for_each(|(handle, mut pty)| {
        let mut buf: Vec<u8> = Vec::new();
        handle.drain_replies_into(&mut buf);
        if !buf.is_empty()
            && let Err(e) = pty.write_all(&buf)
        {
            tracing::warn!(?e, "pty_write reply failed");
        }
    });
}

/// Flushes any coalescer window whose deadline has elapsed. Also
/// rescues the bootstrap snapshot for terminals that have not yet
/// produced PTY output.
fn check_deadline_flush(
    par_commands: ParallelCommands,
    mut q: Query<(Entity, &mut TerminalHandle, &mut Coalescer)>,
) {
    let now = Instant::now();
    q.par_iter_mut()
        .for_each(|(entity, mut handle, mut coalescer)| {
            // NOTE: bootstrap rescue — alacritty's first damage() returns Full
            // even with no chunks yet, so we can emit the Initial snapshot.
            // Daemon handled this implicitly in its wait_deadline arm; the
            // chained-systems port does it explicitly. Required for terminals
            // that don't produce output immediately.
            if handle.needs_bootstrap_emit() {
                handle.force_bootstrap_damage();
                par_commands.command_scope(|mut commands| {
                    handle.emit(&mut commands, entity);
                });
                coalescer.disarm();
                return;
            }
            if let Some(deadline) = coalescer.next_deadline()
                && now >= deadline
            {
                par_commands.command_scope(|mut commands| {
                    handle.emit(&mut commands, entity);
                });
                coalescer.disarm();
            }
        });
}

/// Polls `exit_rx` and fires `TerminalChildExit` once per terminal.
fn drain_pty_exits(par_commands: ParallelCommands, q: Query<(Entity, &PtyHandle)>) {
    q.par_iter().for_each(|(entity, pty)| {
        if let Ok(code) = pty.try_recv_exit() {
            par_commands.command_scope(|mut commands| {
                commands.trigger(TerminalChildExit { entity, code });
            });
        }
    });
}

/// Pulls all available PTY chunks, advances Term, and decides
/// (immediate flush vs. arm) per chunk.
///
/// Three per-chunk paths:
/// - already-adopted terminals buffer raw bytes on `AdoptedControlMode` and
///   never touch the VT;
/// - watched terminals route each chunk through [`scan_handover`]: on
///   `NotYet` the pre-introducer bytes take the normal flush/arm path, and on
///   `Detected` the pre-introducer bytes flush into the VT, the handle is
///   adopted, and [`ControlModeDetected`] fires;
/// - all other terminals take the unchanged normal path.
fn process_pty_chunks(
    par_commands: &ParallelCommands,
    entity: Entity,
    handle: &mut TerminalHandle,
    pty: &mut PtyHandle,
    coalescer: &mut Coalescer,
    mut watch: Option<Mut<ControlModeWatch>>,
    mut adopted: Option<Mut<AdoptedControlMode>>,
) {
    while let Ok(chunk) = pty.try_recv_chunk() {
        if let Some(adopted) = adopted.as_deref_mut() {
            adopted.captured.extend_from_slice(&chunk);
            continue;
        }
        if let Some(watch) = watch.as_deref_mut() {
            match scan_handover(watch, &chunk) {
                Handover::NotYet { vt } => {
                    ingest_and_flush_or_arm(par_commands, entity, handle, coalescer, &vt);
                }
                Handover::Detected { vt, captured } => {
                    let _ = handle.ingest_chunk(&vt, coalescer);
                    par_commands.command_scope(|mut commands| {
                        handle.emit(&mut commands, entity);
                        commands.entity(entity).remove::<ControlModeWatch>();
                        // NOTE: `captured` starts at the introducer byte;
                        // downstream `ProtocolClient::feed` strips it.
                        commands
                            .entity(entity)
                            .insert(AdoptedControlMode { captured });
                        commands.trigger(ControlModeDetected { entity });
                    });
                }
            }
            continue;
        }
        ingest_and_flush_or_arm(par_commands, entity, handle, coalescer, &chunk);
    }
}

/// Feeds `bytes` to the VT and either immediately flushes the emit or arms
/// the coalescer, matching the engine's normal per-chunk flush/arm semantics.
///
/// No-op on empty `bytes`: the handover scanner can withhold an entire chunk
/// as a carried partial-introducer prefix, and feeding zero bytes to the VT
/// must not arm the coalescer or trip the first-emit bootstrap.
fn ingest_and_flush_or_arm(
    par_commands: &ParallelCommands,
    entity: Entity,
    handle: &mut TerminalHandle,
    coalescer: &mut Coalescer,
    bytes: &[u8],
) {
    if bytes.is_empty() {
        return;
    }
    let should_flush = handle.ingest_chunk(bytes, coalescer);
    if should_flush {
        par_commands.command_scope(|mut commands| {
            handle.emit(&mut commands, entity);
        });
        coalescer.disarm();
    } else {
        coalescer.arm_or_extend(Instant::now());
    }
}

/// Drains alacritty control events (Bell / Title / ResetTitle /
/// Clipboard) into Observer triggers and updates the `TerminalTitle`
/// component as a side-effect of Title / ResetTitle.
fn process_control_events(
    par_commands: &ParallelCommands,
    entity: Entity,
    handle: &TerminalHandle,
    title: &mut TerminalTitle,
) {
    par_commands.command_scope(|mut commands| {
        handle.drain_control_events(&mut commands, entity, title);
    });
}

/// Observer for `TerminalKeyInput`. Encodes the key using the entity's
/// `Term::mode()` (for app-cursor-keys lookup) and writes the resulting
/// VT bytes to the PTY via `TerminalHandle::write`, which also sets
/// `pending_user_input = true` so the coalescer immediate-flush path
/// fires on the next PTY chunk.
///
/// If the viewport is scrolled back when the key arrives, the view is
/// snapped to the live tail before forwarding the keystroke to the PTY.
fn on_terminal_key_input(
    ev: On<TerminalKeyInput>,
    mut q: Query<(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer)>,
) {
    let Ok((mut handle, mut pty, mut coalescer)) = q.get_mut(ev.entity) else {
        return;
    };
    let Some(bytes) = encode_key(&ev.key, &ev.modifiers, handle.is_app_cursor_keys()) else {
        return;
    };
    if !handle.is_at_bottom() {
        handle.scroll_to_bottom(&mut coalescer);
    }
    if let Err(e) = handle.write(&mut pty, &bytes) {
        tracing::warn!(?e, entity = ?ev.entity, "terminal key input write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_mode::AdoptedControlMode;
    use crossbeam_channel::{Sender, unbounded};
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    #[derive(Resource, Default)]
    struct DetectedCount(usize);

    fn count_detected(_ev: On<ControlModeDetected>, mut count: ResMut<DetectedCount>) {
        count.0 += 1;
    }

    /// Builds a `PtyHandle` over a real (but otherwise idle) PTY pair so the
    /// constructor's `MasterPty`/writer/killer requirements are satisfied,
    /// while returning a `chunk_tx` the test uses to inject PTY chunks
    /// directly into the handle's chunk channel.
    fn pty_handle_with_injector() -> (PtyHandle, Sender<Vec<u8>>) {
        let pty_pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty pair");
        let child = pty_pair
            .slave
            .spawn_command(CommandBuilder::new("cat"))
            .expect("spawn cat");
        let child_killer = child.clone_killer();
        drop(pty_pair.slave);
        let writer = pty_pair.master.take_writer().expect("take writer");

        let (chunk_tx, chunk_rx) = unbounded::<Vec<u8>>();
        let (_exit_tx, exit_rx) = unbounded::<Option<i32>>();
        let pty = PtyHandle::new(pty_pair.master, writer, chunk_rx, exit_rx, child_killer);
        (pty, chunk_tx)
    }

    #[test]
    fn divert_adopts_on_introducer_and_fires_detected() {
        let mut app = App::new();
        app.add_plugins(TerminalHandlePlugin)
            .init_resource::<DetectedCount>()
            .add_observer(count_detected);

        let handle = TerminalHandle::detached(80, 24, Arc::new(AtomicBool::new(false)));
        let (pty, chunk_tx) = pty_handle_with_injector();
        let entity = app
            .world_mut()
            .spawn((
                handle,
                pty,
                Coalescer::new(),
                TerminalTitle::default(),
                ControlModeWatch::default(),
            ))
            .id();

        chunk_tx
            .send(b"$ tmux -CC\r\n\x1bP1000p%begin 1\r\n".to_vec())
            .expect("inject chunk");
        app.update();

        let world = app.world();
        let adopted = world
            .get::<AdoptedControlMode>(entity)
            .expect("entity must gain AdoptedControlMode after the introducer");
        assert_eq!(
            adopted.captured, b"\x1bP1000p%begin 1\r\n",
            "captured must begin at the introducer byte"
        );
        assert!(
            world.get::<ControlModeWatch>(entity).is_none(),
            "ControlModeWatch must be removed once adopted"
        );
        assert_eq!(
            world.resource::<DetectedCount>().0,
            1,
            "ControlModeDetected must fire exactly once"
        );

        app.world_mut().despawn(entity);
    }

    #[test]
    fn divert_buffers_subsequent_chunks_without_vt() {
        let mut app = App::new();
        app.add_plugins(TerminalHandlePlugin)
            .init_resource::<DetectedCount>()
            .add_observer(count_detected);

        let handle = TerminalHandle::detached(80, 24, Arc::new(AtomicBool::new(false)));
        let (pty, chunk_tx) = pty_handle_with_injector();
        let entity = app
            .world_mut()
            .spawn((
                handle,
                pty,
                Coalescer::new(),
                TerminalTitle::default(),
                ControlModeWatch::default(),
            ))
            .id();

        chunk_tx.send(b"\x1bP1000p%begin\r\n".to_vec()).unwrap();
        app.update();
        chunk_tx.send(b"%output %1 hello\r\n".to_vec()).unwrap();
        app.update();

        let mut adopted = app
            .world_mut()
            .get_mut::<AdoptedControlMode>(entity)
            .expect("adopted");
        assert_eq!(
            adopted.take_captured(),
            b"\x1bP1000p%begin\r\n%output %1 hello\r\n",
            "post-adoption chunks append verbatim, no VT diversion"
        );
        assert_eq!(
            app.world().resource::<DetectedCount>().0,
            1,
            "ControlModeDetected fires once, not per subsequent chunk"
        );

        app.world_mut().despawn(entity);
    }

    #[test]
    fn divert_carries_split_introducer_then_adopts() {
        let mut app = App::new();
        app.add_plugins(TerminalHandlePlugin)
            .init_resource::<DetectedCount>()
            .add_observer(count_detected);

        let handle = TerminalHandle::detached(80, 24, Arc::new(AtomicBool::new(false)));
        let (pty, chunk_tx) = pty_handle_with_injector();
        let entity = app
            .world_mut()
            .spawn((
                handle,
                pty,
                Coalescer::new(),
                TerminalTitle::default(),
                ControlModeWatch::default(),
            ))
            .id();

        chunk_tx.send(b"out\x1bP10".to_vec()).unwrap();
        app.update();
        assert!(
            app.world().get::<AdoptedControlMode>(entity).is_none(),
            "a partial introducer must NOT adopt yet"
        );
        assert_eq!(
            app.world().resource::<DetectedCount>().0,
            0,
            "no detection while the introducer is still split"
        );

        chunk_tx.send(b"00p%begin\r\n".to_vec()).unwrap();
        app.update();
        let adopted = app
            .world()
            .get::<AdoptedControlMode>(entity)
            .expect("second chunk completes the introducer and adopts");
        assert_eq!(
            adopted.captured, b"\x1bP1000p%begin\r\n",
            "captured rejoins the carried introducer prefix"
        );
        assert_eq!(app.world().resource::<DetectedCount>().0, 1);

        app.world_mut().despawn(entity);
    }
}
