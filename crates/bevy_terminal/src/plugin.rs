//! `TerminalHandlePlugin` — registers the chained Bevy systems that
//! drive each terminal's `ozmux_vt::vt::Vt` engine and translate its
//! data-returning output (`Frame` / `VtEvent` / reply bytes) into Bevy
//! `EntityEvent`s and PTY writes.

use crate::events::{
    TerminalBell, TerminalChildExit, TerminalClipboardStore, TerminalCurrentDir, TerminalKeyInput,
    TerminalModeChanged, TerminalTitleChanged,
};
use crate::handle::TerminalHandle;
use crate::input_codec::encode_key;
use crate::pty::PtyHandle;
use crate::title::{TerminalTitle, sanitize_title};
use bevy::ecs::entity::Entity;
use bevy::ecs::observer::On;
use bevy::ecs::system::{Commands, ParallelCommands};
use bevy::prelude::*;
use bevy_terminal_renderer::prelude::{TerminalDelta, TerminalSnapshot};
use ozmux_vt::event::VtEvent;
use ozmux_vt::frame::Frame;
use ozmux_vt::vt::OutputAction;
use std::time::Instant;

/// Adds the terminal bridge systems to the Bevy app's `Update` schedule.
pub struct TerminalHandlePlugin;

impl Plugin for TerminalHandlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (drain_pty_chunks, check_deadline_flush, drain_pty_exits).chain(),
        );
        app.add_observer(on_terminal_key_input);
    }
}

/// Builds the `Frame` into the matching renderer `EntityEvent`.
fn trigger_frame(commands: &mut Commands, entity: Entity, frame: Frame) {
    match frame {
        Frame::Snapshot(snapshot) => commands.trigger(TerminalSnapshot { entity, snapshot }),
        Frame::Delta(delta) => commands.trigger(TerminalDelta { entity, delta }),
    }
}

/// Translates a neutral `VtEvent` into the matching Bevy `EntityEvent`.
/// `TitleChanged` additionally sanitizes the title and updates the
/// entity's `TerminalTitle` component so the displayed title stays
/// attacker-safe.
fn trigger_vt_event(
    commands: &mut Commands,
    title: &mut TerminalTitle,
    entity: Entity,
    ev: VtEvent,
) {
    match ev {
        VtEvent::Bell => commands.trigger(TerminalBell { entity }),
        VtEvent::TitleChanged(Some(raw)) => {
            let sanitized = sanitize_title(&raw);
            title.0 = Some(sanitized.clone());
            commands.trigger(TerminalTitleChanged {
                entity,
                title: Some(sanitized),
            });
        }
        VtEvent::TitleChanged(None) => {
            title.0 = None;
            commands.trigger(TerminalTitleChanged {
                entity,
                title: None,
            });
        }
        VtEvent::ClipboardStore(content) => {
            commands.trigger(TerminalClipboardStore { entity, content });
        }
        VtEvent::CurrentDir(path) => commands.trigger(TerminalCurrentDir { entity, path }),
        VtEvent::ModeChanged { added, removed } => {
            commands.trigger(TerminalModeChanged {
                entity,
                added,
                removed,
            });
        }
    }
}

/// Feeds PTY output into each terminal's `Vt`, emits resulting frames,
/// translates engine events, and writes any reply bytes back to the PTY.
fn drain_pty_chunks(
    par_commands: ParallelCommands,
    mut q: Query<(
        Entity,
        &mut TerminalHandle,
        &mut PtyHandle,
        &mut TerminalTitle,
    )>,
) {
    q.par_iter_mut()
        .for_each(|(entity, mut handle, mut pty, mut title)| {
            process_pty_chunks(&par_commands, entity, &mut handle, &mut pty, &mut title);
        });
}

/// Flushes any coalescer window whose deadline has elapsed, and rescues
/// the bootstrap snapshot for terminals that have not yet produced
/// output. `Vt::tick` folds both behaviours.
fn check_deadline_flush(
    par_commands: ParallelCommands,
    mut q: Query<(Entity, &mut TerminalHandle)>,
) {
    let now = Instant::now();
    q.par_iter_mut().for_each(|(entity, mut handle)| {
        if let Some(frame) = handle.vt.tick(now) {
            par_commands.command_scope(|mut commands| {
                trigger_frame(&mut commands, entity, frame);
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

/// Pulls all available PTY chunks, drives the engine per chunk, then
/// drains engine events and reply bytes.
fn process_pty_chunks(
    par_commands: &ParallelCommands,
    entity: Entity,
    handle: &mut TerminalHandle,
    pty: &mut PtyHandle,
    title: &mut TerminalTitle,
) {
    let now = Instant::now();
    while let Ok(chunk) = pty.try_recv_chunk() {
        if matches!(handle.vt.on_output(&chunk, now), OutputAction::EmitNow)
            && let Some(frame) = handle.vt.emit()
        {
            par_commands.command_scope(|mut commands| {
                trigger_frame(&mut commands, entity, frame);
            });
        }
    }
    // NOTE: VtEvents (incl. ModeChanged) are drained AFTER the per-chunk frame
    // triggers, so a mode change now lands after its frame (the pre-extraction
    // emit triggered TerminalModeChanged before the frame). Harmless today —
    // TerminalModeChanged has no observers — but a future consumer relying on
    // mode-before-frame ordering must move the drain ahead of the emit loop.
    let events = handle.vt.drain_events();
    if !events.is_empty() {
        par_commands.command_scope(|mut commands| {
            for ev in events {
                trigger_vt_event(&mut commands, title, entity, ev);
            }
        });
    }
    // NOTE: replies are produced only by `Vt::on_output` (parser advance),
    // never by `emit`/`tick`, so draining once after the chunk loop captures
    // every reply — `check_deadline_flush` needs no separate reply drain.
    let replies = handle.vt.drain_replies();
    if !replies.is_empty()
        && let Err(e) = pty.write_all(&replies)
    {
        tracing::warn!(?e, "pty_write reply failed");
    }
}

/// Observer for `TerminalKeyInput`. Encodes the key using the entity's
/// `Term::mode()` (for app-cursor-keys lookup) and writes the resulting
/// VT bytes to the PTY via `TerminalHandle::write`, which also sets
/// `pending_user_input` so the coalescer immediate-flush path fires on
/// the next PTY chunk.
///
/// If the viewport is scrolled back when the key arrives, the view is
/// snapped to the live tail before forwarding the keystroke to the PTY.
fn on_terminal_key_input(
    ev: On<TerminalKeyInput>,
    mut q: Query<(&mut TerminalHandle, &mut PtyHandle)>,
) {
    let Ok((mut handle, mut pty)) = q.get_mut(ev.entity) else {
        return;
    };
    let Some(bytes) = encode_key(&ev.key, &ev.modifiers, handle.is_app_cursor_keys()) else {
        return;
    };
    if !handle.is_at_bottom() {
        handle.scroll_to_bottom();
    }
    if let Err(e) = handle.write(&mut pty, &bytes) {
        tracing::warn!(?e, entity = ?ev.entity, "terminal key input write failed");
    }
}
