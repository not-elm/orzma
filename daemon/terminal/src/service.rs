//! Activity-keyed orchestrator over PTY handles and the VT bridge.

use crate::{
    error::{PtyErrorBridge, TerminalError, TerminalResult},
    event::TerminalEvent,
    pty::reader::spawn_pty_reader,
    pty::scrollback::ScrollbackBuffer,
    service::terminal_handle::TerminalHandle,
    service::types::{FrameSubscription, SpawnOptions, TerminalGeometry},
    vt::bridge::VtState,
    vt::frame::{RenderFrame, SnapshotReason, encode},
    vt::frame_builder::build_snapshot,
};
use bytes::Bytes;
use ozmux_extension::runtime::RuntimeRoot;
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{RwLock, RwLockReadGuard, broadcast};

pub(crate) mod terminal_handle;
pub mod types;

/// Activity-keyed registry of running terminals.
///
/// Each entry owns a `TerminalHandle` (PTY + scrollback + VT bridge) and is
/// keyed by [`ActivityId`].
#[derive(Default)]
#[cfg_attr(feature = "bevy", derive(bevy::prelude::Resource))]
pub struct TerminalService {
    handles: HashMap<ActivityId, TerminalHandle>,
    runtime_root: Option<RuntimeRoot>,
}

impl TerminalService {
    /// Constructs a service that exposes the runtime root's `bin/` directory
    /// to spawned shells via `PATH`.
    pub fn with_runtime_root(root: RuntimeRoot) -> Self {
        Self {
            runtime_root: Some(root),
            ..Self::default()
        }
    }

    /// Spawns a shell for `activity_id` under `pane_id` with the given options.
    pub fn spawn(
        &mut self,
        pane_id: PaneId,
        activity_id: ActivityId,
        opts: SpawnOptions,
    ) -> TerminalResult {
        if self.handles.contains_key(&activity_id) {
            return Ok(());
        }

        let pty_pair = native_pty_system()
            .openpty(PtySize {
                rows: opts.rows,
                cols: opts.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .to_terminal_result()?;

        let cmd = self.pty_command_builder(&pane_id, &activity_id, &opts);
        let child = pty_pair.slave.spawn_command(cmd).to_terminal_result()?;
        let killer = child.clone_killer();
        drop(pty_pair.slave);

        let reader = pty_pair.master.try_clone_reader().to_terminal_result()?;
        let writer = pty_pair.master.take_writer().to_terminal_result()?;
        let scrollback = ScrollbackBuffer::new();
        let (event_sender, _) = broadcast::channel(1024);

        let (vt_chunk_tx, vt_chunk_rx) = tokio::sync::mpsc::channel::<Bytes>(512);

        spawn_pty_reader(
            reader,
            child,
            scrollback.clone(),
            event_sender.clone(),
            vt_chunk_tx.clone(),
        );

        let handle = TerminalHandle::new(
            pty_pair.master,
            writer,
            event_sender,
            killer,
            scrollback,
            opts.cols,
            opts.rows,
            vt_chunk_tx,
            vt_chunk_rx,
        );
        self.handles.insert(activity_id, handle);
        Ok(())
    }

    /// Writes raw bytes to the PTY master for `activity`, setting the
    /// pending-user-input flag before the syscall so the bridge cannot miss it.
    pub fn write(&mut self, activity: &ActivityId, data: &[u8]) -> TerminalResult {
        let handle = self.handle_mut(activity)?;
        handle.write(data)?;
        Ok(())
    }

    /// Resizes the PTY and underlying VT grid, then wakes the bridge task so
    /// the resulting Full damage is emitted without waiting for the next chunk.
    pub fn resize(&mut self, activity: &ActivityId, cols: u16, rows: u16) -> TerminalResult {
        self.handle_mut(activity)?.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Removes and drops the handle for `activity`, terminating its bridge
    /// task. Missing activities are logged but not treated as errors.
    pub fn kill(&mut self, activity: &ActivityId) -> TerminalResult {
        match self.handles.remove(activity) {
            Some(h) => {
                drop(h);
            }
            None => {
                tracing::warn!(
                    activity_id = %activity.as_ref(),
                    "TerminalService::kill called for missing activity"
                );
            }
        }
        Ok(())
    }

    /// Returns the current scrollback snapshot and a fresh broadcast receiver
    /// for raw [`TerminalEvent`] emissions, captured under one lock.
    #[inline]
    pub fn snapshot(&self, activity: &ActivityId) -> TerminalResult<Vec<u8>> {
        let handle = self.handle(activity)?;
        Ok(handle.snapshot())
    }

    /// Subscribes to wire frames for the given activity, atomically with respect
    /// to the bridge task's emissions — no frame is dropped or duplicated between
    /// the snapshot/replay capture and the broadcast receiver attaching.
    ///
    /// Both the snapshot read and `wire_broadcast.subscribe()` happen under the
    /// same `vt_state.lock()` that the bridge holds during emission, ensuring the
    /// critical section is either entirely before or entirely after any given emit.
    ///
    /// # Return value
    ///
    /// - `ResumeReplay` when `last_seq` is `Some` and the ring covers the range
    ///   `[last_seq+1, latest]` without gaps.
    /// - `FreshSnapshot { reason: Lagged }` when `last_seq` is `Some` but the
    ///   ring has evicted past that point.
    /// - `FreshSnapshot { reason: Reconnect }` when `last_seq` is `None`
    ///   (cold start or initial connect).
    pub fn subscribe_frames(
        &self,
        activity: &ActivityId,
        last_seq: Option<u32>,
    ) -> TerminalResult<FrameSubscription> {
        let handle = self.handle(activity).await?;
        let vt_state = handle.vt_state.clone();
        drop(handle);

        let mut state = vt_state.lock().expect("vt_state poisoned");

        if let Some(last) = last_seq
            && let Some(deltas) = state.frame_ring.replay(last)
        {
            let rx = state.wire_broadcast.subscribe();
            return Ok(FrameSubscription::ResumeReplay { deltas, rx });
        }

        // Fresh snapshot path. The snapshot is not a sequenced emission, so we
        // do not bump frame_seq. The snapshot carries the seq of the last emitted
        // frame (frame_seq - 1) so that clients know the broadcast rx continues
        // from frame_seq onward (i.e., next_broadcast_seq > snapshot_seq).
        // When no frame has been emitted yet (frame_seq == 0), we use 0 as a
        // sentinel — the client will accept the first broadcast regardless.
        let reason = if last_seq.is_some() {
            SnapshotReason::Lagged
        } else {
            SnapshotReason::Reconnect
        };
        let snap_seq = state.frame_seq.saturating_sub(1);
        let snap = {
            let VtState {
                ref term,
                ref mut hyperlinks,
                ..
            } = *state;
            build_snapshot(term, snap_seq, reason, hyperlinks)
        };
        let encoded_vec =
            encode(&RenderFrame::Snapshot(snap)).expect("encode infallible for valid frame");
        let rx = state.wire_broadcast.subscribe();
        Ok(FrameSubscription::FreshSnapshot {
            snapshot: Bytes::from(encoded_vec),
            rx,
        })
    }

    /// Reads the current geometry and cursor state under the vt_state lock.
    ///
    /// Returns the column count, row count, and cursor position/shape as of
    /// the instant the lock is acquired. Intended for emitting the hello frame
    /// on VT WebSocket connect.
    #[inline]
    pub fn read_geometry(&self, activity: &ActivityId) -> TerminalResult<TerminalGeometry> {
        let geometry = self.handle(activity)?.read_geometry();
        Ok(geometry)
    }

    /// Scrolls the visible viewport by `delta` lines.
    ///
    /// Positive `delta` moves backward into scrollback history; negative
    /// moves forward toward the live tail. Alacritty clamps to `[0, history_size]`.
    ///
    /// Triggers Full damage in alacritty when `display_offset` changes, so the
    /// bridge emits a snapshot through the existing path. The synthetic empty
    /// chunk wakes the bridge task if no PTY output is pending.
    #[inline]
    pub fn scroll(&mut self, activity: &ActivityId, delta: i32) -> TerminalResult {
        self.handle_mut(activity)?.scroll(delta);
        Ok(())
    }

    /// Snaps the viewport back to the live tail and resumes auto-follow.
    #[inline]
    pub async fn scroll_to_bottom(&mut self, activity: &ActivityId) -> TerminalResult {
        self.handle_mut(activity)?.scroll_to_bottom();
        Ok(())
    }

    #[inline]
    fn handle(&self, activity_id: &ActivityId) -> TerminalResult<&TerminalHandle> {
        self.handles
            .get(activity_id)
            .ok_or_else(|| TerminalError::ActivityNotFound(activity_id.clone()))
    }

    #[inline]
    fn handle_mut(&mut self, activity_id: &ActivityId) -> TerminalResult<&mut TerminalHandle> {
        self.handles
            .get_mut(activity_id)
            .ok_or_else(|| TerminalError::ActivityNotFound(activity_id.clone()))
    }

    fn extension_path_prefix(&self) -> Option<String> {
        let root = self.runtime_root.as_ref()?;
        let bin_dir = root.bin_dir();
        let entries: Vec<String> = std::fs::read_dir(bin_dir)
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().ok().is_some_and(|t| t.is_dir()))
            .map(|e| e.path().to_string_lossy().into_owned())
            .collect();
        build_path_prefix(entries)
    }

    fn pty_command_builder(
        &self,
        pane_id: &PaneId,
        activity_id: &ActivityId,
        opts: &SpawnOptions,
    ) -> CommandBuilder {
        let mut cmd = CommandBuilder::new(&opts.shell);
        if let Some(cwd) = &opts.cwd {
            cmd.cwd(cwd);
        }
        cmd.env("OZMUX_PANE_ID", pane_id.as_ref());
        cmd.env("OZMUX_ACTIVITY_ID", activity_id.as_ref());
        if let Some(wid) = &opts.window_id {
            cmd.env("OZMUX_WINDOW_ID", wid.as_ref());
        }
        if let Some(sid) = &opts.session_id {
            cmd.env("OZMUX_SESSION_ID", sid.as_ref());
        }
        if let Some(prefix) = self.extension_path_prefix() {
            let existing = std::env::var("PATH").unwrap_or_default();
            let combined = if existing.is_empty() {
                prefix
            } else {
                format!("{prefix}:{existing}")
            };
            cmd.env("PATH", combined);
        }
        cmd
    }
}

/// Builds the colon-joined PATH prefix from a list of bin dir paths,
/// pinning `__builtin` to the head so built-in shims always win over
/// extensions of the same name. Extracted as a pure free function so
/// the ordering can be unit-tested without spinning up a real
/// `RuntimeRoot`. The name `__builtin` is the canonical reserved bin
/// dir for built-in shims (defined as `BUILTIN_DIR_NAME` in
/// `daemon/bootstrap/src/builtin_commands.rs`); inlined here to
/// avoid a daemon_terminal → daemon_bootstrap dep cycle.
fn build_path_prefix(entries: Vec<String>) -> Option<String> {
    const BUILTIN_DIR_NAME: &str = "__builtin";
    let (mut builtin, mut rest): (Vec<String>, Vec<String>) = entries.into_iter().partition(|p| {
        std::path::Path::new(p).file_name().and_then(|n| n.to_str()) == Some(BUILTIN_DIR_NAME)
    });
    rest.sort();
    builtin.append(&mut rest);
    if builtin.is_empty() {
        None
    } else {
        Some(builtin.join(":"))
    }
}
