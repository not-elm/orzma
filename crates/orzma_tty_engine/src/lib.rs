//! Bevy-native terminal: PTY ownership, alacritty VT emulation, and
//! coalesced `FrameSnapshot` / `FrameDelta` emission against the
//! `orzma_tty_renderer` schema.

mod bundle;
mod buttons;
mod coalescer;
mod events;
mod handle;
mod input_codec;
mod mouse_encode;
mod osc;
mod palette;
mod pty;
mod title;
mod vt;
mod wheel;

pub use alacritty_terminal::index::{Column, Line, Point, Side};
pub use alacritty_terminal::selection::SelectionType;
pub use alacritty_terminal::term::TermMode;
pub use alacritty_terminal::vi_mode::ViMotion;
pub use bundle::{SpawnOptions, TerminalBundle};
pub use buttons::{ButtonAction, ButtonConfig, ButtonEvent, ButtonEventKind, MouseButtonKind};
pub use coalescer::Coalescer;
pub use events::{
    OscWebviewRequest, TerminalBell, TerminalChildExit, TerminalClipboardStore, TerminalCurrentDir,
    TerminalKey, TerminalKeyInput, TerminalModeChanged, TerminalModifiers, TerminalTitleChanged,
};
pub use handle::{TerminalHandle, ViIndicatorSnapshot};
pub use mouse_encode::ProtocolModifiers;
pub use pty::PtyHandle;
pub use title::{TerminalTitle, sanitize_title};
pub use vt::listener::{AnchorMode, InlineAnchor, OscWebviewVerb};
pub use wheel::{CellCoord, WheelAction, WheelConfig, WheelDir, WheelModifiers};

use crate::input_codec::encode_key;
use bevy::ecs::entity::Entity;
use bevy::ecs::observer::On;
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
                flush_due_terminals,
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
    mut commands: Commands,
    mut terminals: Query<(
        Entity,
        &mut TerminalHandle,
        &mut PtyHandle,
        &mut Coalescer,
        &mut TerminalTitle,
    )>,
) {
    for (entity, mut handle, mut pty, mut coalescer, mut title) in terminals.iter_mut() {
        process_pty_chunks(&mut commands, entity, &mut handle, &mut pty, &mut coalescer);
        handle.drain_control_events(&mut commands, entity, &mut title);
    }
}

/// Drains `reply_rx` (alacritty PtyWrite responses) and writes them
/// back to the PTY. Concatenates per-entity into one `write_all` to
/// minimize syscalls.
fn drain_pty_writes(mut terminals: Query<(&TerminalHandle, &mut PtyHandle)>) {
    for (handle, mut pty) in terminals.iter_mut() {
        let mut buf: Vec<u8> = Vec::new();
        handle.drain_replies_into(&mut buf);
        if !buf.is_empty()
            && let Err(e) = pty.write_all(&buf)
        {
            tracing::warn!(?e, "pty_write reply failed");
        }
    }
}

/// Flushes any coalescer window whose deadline has elapsed.
/// Also rescues the bootstrap snapshot for terminals that have not yet
/// produced PTY output.
fn flush_due_terminals(
    mut terminals: Query<(Entity, &mut TerminalHandle, &mut Coalescer)>,
    mut commands: Commands,
) {
    let now = Instant::now();
    for (entity, mut handle, mut coalescer) in terminals.iter_mut() {
        if handle.needs_bootstrap_emit() {
            handle.force_bootstrap_damage();
            handle.emit(&mut commands, entity);
            coalescer.disarm();
            continue;
        }
        if let Some(deadline) = coalescer.next_deadline()
            && now >= deadline
        {
            handle.emit(&mut commands, entity);
            coalescer.disarm();
        }
    }
}

/// Polls `exit_rx` and fires `TerminalChildExit` once per terminal.
fn drain_pty_exits(mut commands: Commands, terminals: Query<(Entity, &PtyHandle)>) {
    for (entity, pty) in terminals.iter() {
        if let Ok(code) = pty.try_recv_exit() {
            commands.trigger(TerminalChildExit { entity, code });
        }
    }
}

/// Pulls all available PTY chunks, advances Term, and decides
/// (immediate flush vs. arm) per chunk.
fn process_pty_chunks(
    commands: &mut Commands,
    entity: Entity,
    handle: &mut TerminalHandle,
    pty: &mut PtyHandle,
    coalescer: &mut Coalescer,
) {
    while let Ok(chunk) = pty.try_recv_chunk() {
        let should_flush = handle.ingest_chunk(&chunk, coalescer);
        if should_flush {
            handle.emit(commands, entity);
            coalescer.disarm();
        } else {
            coalescer.arm_or_extend(Instant::now());
        }
    }
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
