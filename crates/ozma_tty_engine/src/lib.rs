//! Bevy-native terminal: PTY ownership, alacritty VT emulation, and
//! coalesced `FrameSnapshot` / `FrameDelta` emission against the
//! `ozma_tty_renderer` schema.

mod bundle;
mod buttons;
mod coalescer;
mod control_mode;
mod events;
mod handle;
mod input_codec;
mod mouse_encode;
mod osc;
mod palette;
mod pty;
mod raw_write;
mod release;
mod resize;
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
pub use control_mode::{AdoptedControlMode, ControlModeDetected, ControlModeWatch};
pub use events::{
    OscWebviewRequest, TerminalBell, TerminalChildExit, TerminalClipboardStore, TerminalCurrentDir,
    TerminalKey, TerminalKeyInput, TerminalModeChanged, TerminalModifiers, TerminalTitleChanged,
};
pub use handle::{TerminalHandle, ViIndicatorSnapshot};
pub use mouse_encode::ProtocolModifiers;
pub use pty::PtyHandle;
pub use raw_write::{RawWritePlugin, TerminalRawWrite};
pub use release::ReleaseControlMode;
pub use resize::{ResizePlugin, TerminalResize};
pub use title::{TerminalTitle, sanitize_title};
pub use vt::listener::{AnchorMode, InlineAnchor, OscWebviewVerb};
pub use wheel::{CellCoord, WheelAction, WheelConfig, WheelDir, WheelModifiers};

use crate::input_codec::encode_key;
use bevy::ecs::entity::Entity;
use bevy::ecs::observer::On;
use bevy::prelude::*;
use control_mode::Handover;
use release::ControlModeReleasePlugin;
use std::time::Instant;

/// Adds the four-system terminal bridge to the Bevy app's `Update`
/// schedule.
pub struct TerminalHandlePlugin;

impl Plugin for TerminalHandlePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RawWritePlugin, ResizePlugin, ControlModeReleasePlugin))
            .add_systems(
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
        Option<&mut ControlModeWatch>,
        Option<&mut AdoptedControlMode>,
    )>,
) {
    for (entity, mut handle, mut pty, mut coalescer, mut title, watch, adopted) in
        terminals.iter_mut()
    {
        process_pty_chunks(
            &mut commands,
            entity,
            &mut handle,
            &mut pty,
            &mut coalescer,
            watch,
            adopted,
        );
        handle.drain_control_events(&mut commands, entity, &mut title);
    }
}

/// Drains `reply_rx` (alacritty PtyWrite responses) and writes them
/// back to the PTY. Concatenates per-entity into one `write_all` to
/// minimize syscalls.
///
/// NOTE: excludes adopted gateways via `Without<AdoptedControlMode>` — their PTY
/// is the tmux -CC control stream, not a VT, so writing alacritty VT replies into
/// it would corrupt the protocol. Their VT is frozen post-adoption (see
/// `process_pty_chunks`), so no new replies accrue and skipping the drain is safe.
fn drain_pty_writes(
    mut terminals: Query<(&TerminalHandle, &mut PtyHandle), Without<AdoptedControlMode>>,
) {
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
///
/// Three per-chunk paths:
/// - already-adopted terminals buffer raw bytes on `AdoptedControlMode` and
///   never touch the VT;
/// - watched terminals route each chunk through [`Handover::scan`]: on
///   `NotYet` the pre-introducer bytes take the normal flush/arm path, and on
///   `Detected` the pre-introducer bytes flush into the VT, the handle is
///   adopted, and [`ControlModeDetected`] fires;
/// - all other terminals take the unchanged normal path.
fn process_pty_chunks(
    commands: &mut Commands,
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
            match Handover::scan(watch, &chunk) {
                Handover::NotYet { vt } => {
                    ingest_and_flush_or_arm(commands, entity, handle, coalescer, &vt);
                }
                Handover::Detected { vt, mut captured } => {
                    let _ = handle.ingest_chunk(&vt, coalescer);
                    // NOTE: the remove<ControlModeWatch>/insert<AdoptedControlMode>
                    // below are deferred commands, so any further chunk already
                    // queued this frame would re-enter this `watch` branch and
                    // feed its post-introducer protocol bytes to the VT (lost
                    // from the stream). Drain the rest of the frame into
                    // `captured` now so the whole control stream is preserved.
                    while let Ok(more) = pty.try_recv_chunk() {
                        captured.extend_from_slice(&more);
                    }
                    handle.emit(commands, entity);
                    commands.entity(entity).remove::<ControlModeWatch>();
                    // NOTE: `captured` starts at the introducer byte;
                    // downstream `ProtocolClient::feed` strips it.
                    commands
                        .entity(entity)
                        .insert(AdoptedControlMode { captured });
                    commands.trigger(ControlModeDetected { entity });
                    return;
                }
            }
            continue;
        }
        ingest_and_flush_or_arm(commands, entity, handle, coalescer, &chunk);
    }
}

/// Feeds `bytes` to the VT and either immediately flushes the emit or arms
/// the coalescer, matching the engine's normal per-chunk flush/arm semantics.
///
/// No-op on empty `bytes`: the handover scanner can withhold an entire chunk
/// as a carried partial-introducer prefix, and feeding zero bytes to the VT
/// must not arm the coalescer or trip the first-emit bootstrap.
fn ingest_and_flush_or_arm(
    commands: &mut Commands,
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
        handle.emit(commands, entity);
        coalescer.disarm();
    } else {
        coalescer.arm_or_extend(Instant::now());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_mode::AdoptedControlMode;
    use crossbeam_channel::{Sender, unbounded};
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

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

        let handle = TerminalHandle::detached(80, 24);
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

        let handle = TerminalHandle::detached(80, 24);
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
    fn divert_captures_subsequent_same_frame_chunks() {
        // Regression: tmux's initial burst can arrive as 2+ PTY reads in ONE
        // frame. The introducer chunk adopts, but remove<ControlModeWatch> /
        // insert<AdoptedControlMode> are deferred commands — so the in-loop drain
        // must pull the rest of the frame into `captured`, or those later chunks
        // re-enter the `watch` branch and their protocol bytes go to the VT
        // (lost), desyncing the parser and blanking the projection.
        let mut app = App::new();
        app.add_plugins(TerminalHandlePlugin)
            .init_resource::<DetectedCount>()
            .add_observer(count_detected);

        let handle = TerminalHandle::detached(80, 24);
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

        // Both chunks queued BEFORE a single update -> same-frame draining.
        chunk_tx
            .send(b"\x1bP1000p%begin 1 1 1\r\n".to_vec())
            .unwrap();
        chunk_tx
            .send(b"%output %1 hi\r\n%end 1 1 1\r\n".to_vec())
            .unwrap();
        app.update();

        let mut adopted = app
            .world_mut()
            .get_mut::<AdoptedControlMode>(entity)
            .expect("adopted");
        assert_eq!(
            adopted.take_captured(),
            b"\x1bP1000p%begin 1 1 1\r\n%output %1 hi\r\n%end 1 1 1\r\n",
            "every same-frame chunk after the introducer must be captured, not lost to the VT"
        );
        assert_eq!(app.world().resource::<DetectedCount>().0, 1);

        app.world_mut().despawn(entity);
    }

    #[test]
    fn divert_carries_split_introducer_then_adopts() {
        let mut app = App::new();
        app.add_plugins(TerminalHandlePlugin)
            .init_resource::<DetectedCount>()
            .add_observer(count_detected);

        let handle = TerminalHandle::detached(80, 24);
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

    /// Exercises the empty-`vt` guard in `ingest_and_flush_or_arm`: a chunk that
    /// is EXACTLY a proper introducer prefix yields `NotYet { vt: b"" }`, so the
    /// VT path is handed zero bytes. The guard must keep that from arming the
    /// coalescer or otherwise perturbing the bridge while the introducer is
    /// still incomplete; the next chunk completes it and adoption fires.
    ///
    /// The terminal is primed with a normal line first (so `first_emit` is
    /// false and the coalescer has settled disarmed) — that makes
    /// `coalescer.is_armed()` a discriminating assertion: WITHOUT the guard the
    /// empty-`vt` `ingest_chunk(&[])` classifies empty damage, does not flush
    /// (no pending user input), and arms the coalescer. The `FrameSnapshot`
    /// count is asserted too but is only a weak proxy here — on a non-fresh
    /// idle terminal the no-op emit and the bootstrap rescue both suppress an
    /// extra snapshot regardless of the guard, so `is_armed()` is the assertion
    /// that genuinely fails without the fix.
    #[test]
    fn divert_empty_vt_prefix_does_not_arm_coalescer_then_adopts() {
        use ozma_tty_renderer::schema::FrameSnapshot;

        #[derive(Resource, Default)]
        struct SnapshotCount(usize);

        let mut app = App::new();
        app.add_plugins(TerminalHandlePlugin)
            .init_resource::<DetectedCount>()
            .init_resource::<SnapshotCount>()
            .add_observer(count_detected)
            .add_observer(|_ev: On<FrameSnapshot>, mut count: ResMut<SnapshotCount>| {
                count.0 += 1;
            });

        let handle = TerminalHandle::detached(80, 24);
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

        // Prime: render one normal line so the terminal is non-fresh
        // (first_emit == false) and the coalescer settles disarmed.
        chunk_tx.send(b"$ tmux -CC\r\n".to_vec()).unwrap();
        app.update();
        let primed_snapshots = app.world().resource::<SnapshotCount>().0;
        assert!(
            !app.world().get::<Coalescer>(entity).unwrap().is_armed(),
            "coalescer must be disarmed after the primed line emits"
        );

        // Feed EXACTLY a proper introducer prefix -> NotYet { vt: b"" }, carry
        // is the full 6-byte prefix.
        chunk_tx.send(b"\x1bP1000".to_vec()).unwrap();
        app.update();

        assert!(
            app.world().get::<ControlModeWatch>(entity).is_some(),
            "still watching while the introducer is incomplete"
        );
        assert!(
            app.world().get::<AdoptedControlMode>(entity).is_none(),
            "a bare introducer prefix must NOT adopt yet"
        );
        assert_eq!(
            app.world().resource::<DetectedCount>().0,
            0,
            "no detection on a bare introducer prefix"
        );
        assert!(
            !app.world().get::<Coalescer>(entity).unwrap().is_armed(),
            "empty-vt guard: feeding zero bytes must NOT arm the coalescer"
        );
        assert_eq!(
            app.world().resource::<SnapshotCount>().0,
            primed_snapshots,
            "empty-vt guard: no extra snapshot for a withheld-only chunk"
        );

        // Completion chunk closes the introducer and carries the first block.
        chunk_tx
            .send(b"p%begin 1 1 1\r\n%end 1 1 1\r\n".to_vec())
            .unwrap();
        app.update();

        let adopted = app
            .world()
            .get::<AdoptedControlMode>(entity)
            .expect("completion chunk adopts");
        assert_eq!(
            adopted.captured, b"\x1bP1000p%begin 1 1 1\r\n%end 1 1 1\r\n",
            "captured rejoins the carried prefix and starts at the introducer"
        );
        assert!(app.world().get::<ControlModeWatch>(entity).is_none());
        assert_eq!(app.world().resource::<DetectedCount>().0, 1);

        app.world_mut().despawn(entity);
    }
}
