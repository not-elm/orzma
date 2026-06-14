//! `TerminalHandlePlugin` — registers the 4 chained Bevy systems.

use crate::coalescer::Coalescer;
use crate::events::{TerminalChildExit, TerminalKeyInput};
use crate::handle::TerminalHandle;
use crate::input_codec::encode_key;
use crate::pty::PtyHandle;
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
        app.add_systems(
            Update,
            (
                drain_pty_chunks,
                drain_pty_writes,
                check_deadline_flush,
                drain_pty_exits,
            )
                .chain(),
        );
        app.add_observer(on_terminal_key_input);
    }
}

/// Drains PTY output into alacritty `Term`, classifies damage, and
/// either immediately flushes or arms the coalescer.
///
/// Also drains control events (Bell/Title/ResetTitle/Clipboard)
/// produced by the listener while parsing the chunks.
fn drain_pty_chunks(
    par_commands: ParallelCommands,
    mut q: Query<(
        Entity,
        &mut TerminalHandle,
        &mut PtyHandle,
        &mut Coalescer,
        &mut TerminalTitle,
    )>,
) {
    q.par_iter_mut()
        .for_each(|(entity, mut handle, mut pty, mut coalescer, mut title)| {
            process_pty_chunks(&par_commands, entity, &mut handle, &mut pty, &mut coalescer);
            process_control_events(&par_commands, entity, &handle, &mut title);
        });
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
                    handle.emit(&mut commands, entity, &mut coalescer);
                });
                return;
            }
            if let Some(deadline) = coalescer.next_deadline()
                && now >= deadline
            {
                par_commands.command_scope(|mut commands| {
                    handle.emit(&mut commands, entity, &mut coalescer);
                });
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
fn process_pty_chunks(
    par_commands: &ParallelCommands,
    entity: Entity,
    handle: &mut TerminalHandle,
    pty: &mut PtyHandle,
    coalescer: &mut Coalescer,
) {
    while let Ok(chunk) = pty.try_recv_chunk() {
        let should_flush = handle.ingest_chunk(&chunk, coalescer);
        if should_flush {
            par_commands.command_scope(|mut commands| {
                handle.emit(&mut commands, entity, coalescer);
            });
        } else {
            coalescer.arm_or_extend(Instant::now());
        }
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
