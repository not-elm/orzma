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
use ozmux_multiplexer::{ActivityId, PaneId};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, broadcast};

pub(crate) mod terminal_handle;
pub mod types;

/// Shared inner state of [`TerminalService`], protected by a `tokio::sync::Mutex`
/// to allow async lock acquisition without blocking the executor.
#[derive(Default)]
struct Inner {
    handles: HashMap<ActivityId, TerminalHandle>,
    runtime_root: Option<Arc<RuntimeRoot>>,
}

/// Activity-keyed registry of running terminals.
///
/// Each entry owns a `TerminalHandle` (PTY + scrollback + VT bridge) and is
/// keyed by [`ActivityId`]. The service is cheaply cloneable — clones share
/// the same underlying handle map via `Arc`.
#[derive(Clone, Default)]
#[cfg_attr(feature = "bevy", derive(bevy::prelude::Resource))]
pub struct TerminalService {
    inner: Arc<Mutex<Inner>>,
}

impl TerminalService {
    /// Constructs a service that exposes the runtime root's `bin/` directory
    /// to spawned shells via `PATH`.
    pub fn with_runtime_root(root: Arc<RuntimeRoot>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                handles: HashMap::new(),
                runtime_root: Some(root),
            })),
        }
    }

    /// Spawns a shell for `activity_id` under `pane_id` with the given options.
    pub async fn spawn(
        &self,
        pane_id: PaneId,
        activity_id: ActivityId,
        opts: SpawnOptions,
    ) -> TerminalResult {
        let mut inner = self.inner.lock().await;
        if inner.handles.contains_key(&activity_id) {
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

        let cmd = build_pty_command(&pane_id, &activity_id, &opts, inner.runtime_root.as_deref());
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
        inner.handles.insert(activity_id, handle);
        Ok(())
    }

    /// Returns a scrollback snapshot and a new broadcast receiver for
    /// [`TerminalEvent`] emissions from the activity's PTY.
    pub async fn snapshot_and_subscribe(
        &self,
        activity: &ActivityId,
    ) -> TerminalResult<(Vec<u8>, broadcast::Receiver<TerminalEvent>)> {
        let inner = self.inner.lock().await;
        let handle = inner
            .handles
            .get(activity)
            .ok_or_else(|| TerminalError::ActivityNotFound(activity.clone()))?;
        let snap = handle.snapshot();
        let rx = handle.subscribe_events();
        Ok((snap, rx))
    }

    /// Writes raw bytes to the PTY master for `activity`, setting the
    /// pending-user-input flag before the syscall so the bridge cannot miss it.
    pub async fn write(&self, activity: &ActivityId, data: &[u8]) -> TerminalResult {
        let mut inner = self.inner.lock().await;
        let handle = inner
            .handles
            .get_mut(activity)
            .ok_or_else(|| TerminalError::ActivityNotFound(activity.clone()))?;
        handle.write(data)?;
        Ok(())
    }

    /// Resizes the PTY and underlying VT grid, then wakes the bridge task so
    /// the resulting Full damage is emitted without waiting for the next chunk.
    pub async fn resize(&self, activity: &ActivityId, cols: u16, rows: u16) -> TerminalResult {
        let mut inner = self.inner.lock().await;
        let handle = inner
            .handles
            .get_mut(activity)
            .ok_or_else(|| TerminalError::ActivityNotFound(activity.clone()))?;
        handle.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Removes and drops the handle for `activity`, terminating its bridge
    /// task. Missing activities are logged but not treated as errors.
    pub async fn kill(&self, activity: &ActivityId) -> TerminalResult {
        let mut inner = self.inner.lock().await;
        match inner.handles.remove(activity) {
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

    /// Returns the current scrollback snapshot.
    #[inline]
    pub async fn snapshot(&self, activity: &ActivityId) -> TerminalResult<Vec<u8>> {
        let inner = self.inner.lock().await;
        let handle = inner
            .handles
            .get(activity)
            .ok_or_else(|| TerminalError::ActivityNotFound(activity.clone()))?;
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
    pub async fn subscribe_frames(
        &self,
        activity: &ActivityId,
        last_seq: Option<u32>,
    ) -> TerminalResult<FrameSubscription> {
        let inner = self.inner.lock().await;
        let handle = inner
            .handles
            .get(activity)
            .ok_or_else(|| TerminalError::ActivityNotFound(activity.clone()))?;
        let vt_state = handle.vt_state.clone();
        drop(inner);

        let mut state = vt_state.lock().expect("vt_state poisoned");

        if let Some(last) = last_seq
            && let Some(deltas) = state.frame_ring.replay(last)
        {
            let rx = state.wire_broadcast.subscribe();
            return Ok(FrameSubscription::ResumeReplay { deltas, rx });
        }

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
    pub async fn read_geometry(&self, activity: &ActivityId) -> TerminalResult<TerminalGeometry> {
        let inner = self.inner.lock().await;
        let handle = inner
            .handles
            .get(activity)
            .ok_or_else(|| TerminalError::ActivityNotFound(activity.clone()))?;
        Ok(handle.read_geometry())
    }

    /// Scrolls the visible viewport by `delta` lines.
    ///
    /// Positive `delta` moves backward into scrollback history; negative
    /// moves forward toward the live tail. Alacritty clamps to `[0, history_size]`.
    #[inline]
    pub async fn scroll(&self, activity: &ActivityId, delta: i32) -> TerminalResult {
        let mut inner = self.inner.lock().await;
        let handle = inner
            .handles
            .get_mut(activity)
            .ok_or_else(|| TerminalError::ActivityNotFound(activity.clone()))?;
        handle.scroll(delta);
        Ok(())
    }

    /// Snaps the viewport back to the live tail and resumes auto-follow.
    #[inline]
    pub async fn scroll_to_bottom(&self, activity: &ActivityId) -> TerminalResult {
        let mut inner = self.inner.lock().await;
        let handle = inner
            .handles
            .get_mut(activity)
            .ok_or_else(|| TerminalError::ActivityNotFound(activity.clone()))?;
        handle.scroll_to_bottom();
        Ok(())
    }

    /// Returns all current terminal titles, keyed by activity id.
    pub async fn all_titles(&self) -> HashMap<ActivityId, String> {
        // NOTE: Hold the outer tokio::Mutex only long enough to clone the
        // per-handle Arc<Mutex<VtState>>. Releasing the outer lock before
        // iterating prevents `all_titles` from serializing every other
        // TerminalService op (write/resize/scroll/kill/etc.) behind a routine
        // status-bar poll.
        let snapshots: Vec<(ActivityId, Arc<std::sync::Mutex<crate::vt::bridge::VtState>>)> = {
            let inner = self.inner.lock().await;
            inner
                .handles
                .iter()
                .map(|(aid, h)| (aid.clone(), h.vt_state.clone()))
                .collect()
        };
        snapshots
            .into_iter()
            .filter_map(|(aid, vt_state)| {
                let state = vt_state.lock().expect("vt_state poisoned");
                state.title.clone().map(|t| (aid, t))
            })
            .collect()
    }
}

/// Constructs a [`CommandBuilder`] for the PTY, setting environment variables
/// including `OZMUX_PANE_ID`, `OZMUX_ACTIVITY_ID`, `OZMUX_SESSION_ID`, and
/// an augmented `PATH` if the runtime root is present.
fn build_pty_command(
    pane_id: &PaneId,
    activity_id: &ActivityId,
    opts: &SpawnOptions,
    runtime_root: Option<&RuntimeRoot>,
) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(&opts.shell);
    if let Some(cwd) = &opts.cwd {
        cmd.cwd(cwd);
    }
    cmd.env("OZMUX_PANE_ID", pane_id.as_ref());
    cmd.env("OZMUX_ACTIVITY_ID", activity_id.as_ref());
    if let Some(sid) = &opts.session_id {
        cmd.env("OZMUX_SESSION_ID", sid.to_string());
    }
    if let Some(prefix) = extension_path_prefix(runtime_root) {
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

/// Reads the bin subdirectories under `runtime_root` and builds a colon-joined
/// PATH prefix, pinning `__builtin` first so built-in shims always win.
fn extension_path_prefix(runtime_root: Option<&RuntimeRoot>) -> Option<String> {
    let root = runtime_root?;
    let bin_dir = root.bin_dir();
    let entries: Vec<String> = std::fs::read_dir(bin_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().ok().is_some_and(|t| t.is_dir()))
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    build_path_prefix(entries)
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
