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
#[cfg(any(test, feature = "test-helpers"))]
use std::collections::HashSet;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{RwLock, RwLockReadGuard, broadcast};

pub(crate) mod terminal_handle;
#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers;
pub mod types;

/// Activity-keyed registry of running terminals.
///
/// Each entry owns a `TerminalHandle` (PTY + scrollback + VT bridge) and is
/// keyed by [`ActivityId`].
#[derive(Clone)]
pub struct TerminalService {
    ptys: Arc<RwLock<HashMap<ActivityId, TerminalHandle>>>,
    runtime_root: Option<Arc<RuntimeRoot>>,
    title_tx: broadcast::Sender<WindowId>,
    #[cfg(any(test, feature = "test-helpers"))]
    forced_failures: Arc<RwLock<HashSet<ActivityId>>>,
}

impl Default for TerminalService {
    fn default() -> Self {
        Self {
            ptys: Arc::default(),
            runtime_root: None,
            title_tx: broadcast::channel(256).0,
            #[cfg(any(test, feature = "test-helpers"))]
            forced_failures: Arc::default(),
        }
    }
}

impl TerminalService {
    /// Constructs a service that exposes the runtime root's `bin/` directory
    /// to spawned shells via `PATH`.
    pub fn with_runtime_root(root: Arc<RuntimeRoot>) -> Self {
        Self {
            runtime_root: Some(root),
            ..Self::default()
        }
    }

    /// Spawns a shell for `activity_id` under `pane_id` with the given options.
    pub async fn spawn(
        &self,
        pane_id: PaneId,
        activity_id: ActivityId,
        opts: SpawnOptions,
    ) -> TerminalResult {
        // NOTE: check forced_failures BEFORE the ptys lock so the two locks are
        // never held simultaneously. Consuming the failure here also takes
        // precedence over the dup-key short-circuit below.
        #[cfg(any(test, feature = "test-helpers"))]
        {
            let mut forced = self.forced_failures.write().await;
            if forced.remove(&activity_id) {
                return Err(TerminalError::Pty("forced test failure".into()));
            }
        }

        let mut ptys = self.ptys.write().await;
        if ptys.contains_key(&activity_id) {
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
        let child = pty_pair.slave.spawn_command(cmd).to_terminal_result()?;
        let killer = child.clone_killer();
        drop(pty_pair.slave);

        let reader = pty_pair.master.try_clone_reader().to_terminal_result()?;
        let writer = pty_pair.master.take_writer().to_terminal_result()?;
        let scrollback = ScrollbackBuffer::new();
        let (event_sender, _) = broadcast::channel(1024);

        let (vt_chunk_tx, vt_chunk_rx) = tokio::sync::mpsc::channel::<Bytes>(128);

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
            opts.window_id.clone(),
            self.title_tx.clone(),
        );
        ptys.insert(activity_id, handle);
        Ok(())
    }

    /// Writes raw bytes to the PTY master for `activity`, setting the
    /// pending-user-input flag before the syscall so the bridge cannot miss it.
    pub async fn write(&self, activity: &ActivityId, data: &[u8]) -> TerminalResult {
        let handle = self.read(activity).await?;
        // NOTE: flag is set BEFORE the PTY write so a racing bridge cycle
        // observing this user input cannot miss the flag — the bridge sees
        // either an empty PTY (no chunk yet, flag set) or a chunk plus flag.
        {
            let mut state = handle.vt_state.lock().expect("vt_state poisoned");
            state.pending_user_input = true;
        }
        handle
            .writer
            .lock()
            .await
            .write_all(data)
            .map_err(|e| TerminalError::Pty(e.to_string()))?;
        Ok(())
    }

    /// Resizes the PTY and underlying VT grid, then wakes the bridge task so
    /// the resulting Full damage is emitted without waiting for the next chunk.
    pub async fn resize(&self, activity: &ActivityId, cols: u16, rows: u16) -> TerminalResult {
        let handle = self.read(activity).await?;

        {
            let dim = crate::vt::bridge::dim_for(cols, rows);
            let mut state = handle.vt_state.lock().expect("vt_state poisoned");
            state.term.resize(dim);
        }

        handle
            .master
            .lock()
            .await
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .to_terminal_result()?;

        // NOTE: send().await (not try_send) so a backpressured channel still
        // delivers the wakeup — otherwise the resize-induced Full damage waits
        // until the next genuine PTY chunk, which a non-TUI shell may not
        // produce on SIGWINCH.
        let _ = handle.vt_chunk_tx.send(Bytes::new()).await;

        Ok(())
    }

    /// Removes and drops the handle for `activity`, terminating its bridge
    /// task. Missing activities are logged but not treated as errors.
    pub async fn kill(&self, activity: &ActivityId) -> TerminalResult {
        match self.ptys.write().await.remove(activity) {
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

    /// Subscribes to window-scoped terminal title-change notifications.
    /// Each item is the `WindowId` of a window whose terminal title changed.
    pub fn subscribe_title_changes(
        &self,
    ) -> broadcast::Receiver<WindowId> {
        self.title_tx.subscribe()
    }

    /// Snapshots the current sanitized title of every running terminal.
    /// Activities with no title set are omitted.
    pub async fn all_titles(&self) -> HashMap<ActivityId, String> {
        // NOTE: ptys lock is always acquired before vt_state locks. The bridge
        // holds vt_state only briefly and never touches ptys, so this ordering
        // cannot deadlock.
        let ptys = self.ptys.read().await;
        ptys.iter()
            .filter_map(|(aid, handle)| {
                let title = handle
                    .vt_state
                    .lock()
                    .expect("vt_state poisoned")
                    .title
                    .clone();
                title.map(|t| (aid.clone(), t))
            })
            .collect()
    }

    /// Returns the current scrollback snapshot and a fresh broadcast receiver
    /// for raw [`TerminalEvent`] emissions, captured under one lock.
    pub async fn snapshot_and_subscribe(
        &self,
        activity: &ActivityId,
    ) -> TerminalResult<(Vec<u8>, broadcast::Receiver<TerminalEvent>)> {
        let handle = self.read(activity).await?;
        Ok(handle.snapshot_and_subscribe().await)
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
        let handle = self.read(activity).await?;
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
    pub async fn read_geometry(&self, activity: &ActivityId) -> TerminalResult<TerminalGeometry> {
        use alacritty_terminal::grid::Dimensions;
        let handle = self.read(activity).await?;
        let vt_state = handle.vt_state.clone();
        drop(handle);
        let state = vt_state.lock().expect("vt_state poisoned");
        Ok(TerminalGeometry {
            cols: state.term.columns() as u16,
            rows: state.term.screen_lines() as u16,
            cursor: crate::vt::frame_builder::extract_cursor(&state.term),
        })
    }

    /// Scrolls the visible viewport by `delta` lines.
    ///
    /// Positive `delta` moves backward into scrollback history; negative
    /// moves forward toward the live tail. Alacritty clamps to `[0, history_size]`.
    ///
    /// Triggers Full damage in alacritty when `display_offset` changes, so the
    /// bridge emits a snapshot through the existing path. The synthetic empty
    /// chunk wakes the bridge task if no PTY output is pending.
    pub async fn scroll(&self, activity: &ActivityId, delta: i32) -> TerminalResult {
        let handle = self.read(activity).await?;
        {
            let mut state = handle.vt_state.lock().expect("vt_state poisoned");
            state
                .term
                .scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
        }
        // NOTE: send().await (not try_send) — matches resize semantics so the
        // wakeup survives a backpressured channel.
        let _ = handle.vt_chunk_tx.send(Bytes::new()).await;
        Ok(())
    }

    /// Snaps the viewport back to the live tail and resumes auto-follow.
    pub async fn scroll_to_bottom(&self, activity: &ActivityId) -> TerminalResult {
        let handle = self.read(activity).await?;
        {
            let mut state = handle.vt_state.lock().expect("vt_state poisoned");
            state
                .term
                .scroll_display(alacritty_terminal::grid::Scroll::Bottom);
        }
        let _ = handle.vt_chunk_tx.send(Bytes::new()).await;
        Ok(())
    }

    #[inline]
    async fn read(
        &self,
        activity_id: &ActivityId,
    ) -> TerminalResult<RwLockReadGuard<'_, TerminalHandle>> {
        let guard = self.ptys.read().await;
        RwLockReadGuard::try_map(guard, |ptys| ptys.get(activity_id))
            .map_err(|_| TerminalError::ActivityNotFound(activity_id.clone()))
    }

    fn extension_path_prefix(&self) -> Option<String> {
        let root = self.runtime_root.as_ref()?;
        let bin_dir = root.bin_dir();
        let mut entries: Vec<String> = std::fs::read_dir(bin_dir)
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().ok().is_some_and(|t| t.is_dir()))
            .map(|e| e.path().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        if entries.is_empty() {
            None
        } else {
            Some(entries.join(":"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_multiplexer::{SessionId, WindowId};

    #[tokio::test]
    async fn snapshot_and_subscribe_returns_err_for_unknown_activity() {
        let svc = TerminalService::default();
        let id = ActivityId::new();
        let result = svc.snapshot_and_subscribe(&id).await;
        assert!(matches!(result, Err(TerminalError::ActivityNotFound(ref got)) if got == &id));
    }

    #[tokio::test]
    async fn spawn_then_snapshot_and_subscribe_succeeds() {
        let svc = TerminalService::default();
        let id = ActivityId::new();
        let pane_id = PaneId::new();
        svc.spawn(
            pane_id,
            id.clone(),
            SpawnOptions {
                cols: 80,
                rows: 24,
                shell: "/bin/sh".to_string(),
                cwd: None,
                window_id: None,
                session_id: None,
            },
        )
        .await
        .unwrap();

        let (snap, _rx) = svc.snapshot_and_subscribe(&id).await.unwrap();
        // snapshot may be empty depending on shell startup speed; just confirm Ok.
        let _ = snap;

        // Cleanup
        svc.kill(&id).await.unwrap();
    }

    #[tokio::test]
    async fn spawn_injects_pane_and_activity_ids_into_shell_env() {
        let svc = TerminalService::default();
        let activity_id = ActivityId::new();
        let pane_id = PaneId::new();
        svc.spawn(
            pane_id.clone(),
            activity_id.clone(),
            SpawnOptions {
                cols: 80,
                rows: 24,
                shell: "/bin/sh".to_string(),
                cwd: None,
                window_id: None,
                session_id: None,
            },
        )
        .await
        .unwrap();

        let (_snap, mut rx) = svc.snapshot_and_subscribe(&activity_id).await.unwrap();

        svc.write(
            &activity_id,
            b"printf 'PANE=%s ACT=%s\\n' \"$OZMUX_PANE_ID\" \"$OZMUX_ACTIVITY_ID\"\n",
        )
        .await
        .unwrap();

        let needle = format!("PANE={} ACT={}", pane_id.as_ref(), activity_id.as_ref());

        let mut got = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(TerminalEvent::Data { buffer })) => {
                    got.extend_from_slice(&buffer);
                    if got.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                        break;
                    }
                }
                Ok(Ok(TerminalEvent::Exit { .. })) => break,
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }
        svc.kill(&activity_id).await.unwrap();
        let s = String::from_utf8_lossy(&got);
        assert!(s.contains(&needle), "expected {needle}, got: {s}");
    }

    #[tokio::test]
    async fn spawn_injects_window_and_session_ids_when_provided() {
        let svc = TerminalService::default();
        let activity_id = ActivityId::new();
        let pane_id = PaneId::new();
        let window_id = WindowId::new();
        let session_id = SessionId::new();
        svc.spawn(
            pane_id,
            activity_id.clone(),
            SpawnOptions {
                cols: 80,
                rows: 24,
                shell: "/bin/sh".to_string(),
                cwd: None,
                window_id: Some(window_id.clone()),
                session_id: Some(session_id.clone()),
            },
        )
        .await
        .unwrap();

        let (_snap, mut rx) = svc.snapshot_and_subscribe(&activity_id).await.unwrap();

        svc.write(
            &activity_id,
            b"printf 'WIN=%s SES=%s\\n' \"$OZMUX_WINDOW_ID\" \"$OZMUX_SESSION_ID\"\n",
        )
        .await
        .unwrap();

        let needle = format!("WIN={} SES={}", window_id.as_ref(), session_id.as_ref());
        let mut got = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(TerminalEvent::Data { buffer })) => {
                    got.extend_from_slice(&buffer);
                    if got.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                        break;
                    }
                }
                Ok(Ok(TerminalEvent::Exit { .. })) => break,
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }
        svc.kill(&activity_id).await.unwrap();
        let s = String::from_utf8_lossy(&got);
        assert!(s.contains(&needle), "expected {needle}, got: {s}");
    }

    #[tokio::test]
    async fn pty_output_is_broadcast_to_subscribers() {
        let svc = TerminalService::default();
        let id = ActivityId::new();
        let pane_id = PaneId::new();
        svc.spawn(
            pane_id,
            id.clone(),
            SpawnOptions {
                cols: 80,
                rows: 24,
                shell: "/bin/sh".to_string(),
                cwd: None,
                window_id: None,
                session_id: None,
            },
        )
        .await
        .unwrap();

        let (_snap, mut rx) = svc.snapshot_and_subscribe(&id).await.unwrap();

        // Trigger output: send a known string and read it back from broadcast.
        svc.write(&id, b"echo race_free_marker\n").await.unwrap();

        let mut got = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(TerminalEvent::Data { buffer })) => {
                    got.extend_from_slice(&buffer);
                    if got
                        .windows(b"race_free_marker".len())
                        .any(|w| w == b"race_free_marker")
                    {
                        break;
                    }
                }
                Ok(Ok(TerminalEvent::Exit { .. })) => break,
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }
        svc.kill(&id).await.unwrap();

        let s = String::from_utf8_lossy(&got);
        assert!(
            s.contains("race_free_marker"),
            "expected marker in output, got: {s}"
        );
    }

    #[tokio::test]
    async fn spawn_with_runtime_root_prepends_path() {
        use ozmux_extension::runtime::RuntimeRoot;
        use std::sync::Arc;

        let parent = tempfile::tempdir().unwrap();
        let rt = Arc::new(RuntimeRoot::new_in(parent.path(), std::process::id()).unwrap());
        let ext_bin = rt.bin_dir().join("memo");
        std::fs::create_dir_all(&ext_bin).unwrap();

        let svc = TerminalService::with_runtime_root(Arc::clone(&rt));
        let activity_id = ActivityId::new();
        let pane_id = PaneId::new();
        svc.spawn(
            pane_id,
            activity_id.clone(),
            SpawnOptions {
                cols: 80,
                rows: 24,
                shell: "/bin/sh".to_string(),
                cwd: None,
                window_id: None,
                session_id: None,
            },
        )
        .await
        .unwrap();

        let (_snap, mut rx) = svc.snapshot_and_subscribe(&activity_id).await.unwrap();
        svc.write(&activity_id, b"echo PATHHEAD=\"$PATH\"\n")
            .await
            .unwrap();

        let needle = format!("PATHHEAD={}", ext_bin.display());
        let mut got = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(TerminalEvent::Data { buffer })) => {
                    got.extend_from_slice(&buffer);
                    if got.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                        break;
                    }
                }
                Ok(Ok(TerminalEvent::Exit { .. })) => break,
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }
        svc.kill(&activity_id).await.unwrap();
        let s = String::from_utf8_lossy(&got);
        assert!(
            s.contains(&needle),
            "expected `{needle}` in output, got: {s}"
        );
    }

    #[tokio::test]
    async fn runtime_root_arc_keeps_tree_alive_until_service_dropped() {
        use ozmux_extension::runtime::RuntimeRoot;
        use std::sync::Arc;
        let parent = tempfile::tempdir().unwrap();
        let rt = Arc::new(RuntimeRoot::new_in(parent.path(), std::process::id()).unwrap());
        let path = rt.root().to_path_buf();
        let svc = TerminalService::with_runtime_root(Arc::clone(&rt));
        drop(rt);
        assert!(path.exists(), "Arc inside service keeps RuntimeRoot alive");
        drop(svc);
        assert!(
            !path.exists(),
            "service drop releases last Arc → RuntimeRoot Drop"
        );
    }

    #[tokio::test]
    async fn runtime_root_skips_when_empty() {
        use ozmux_extension::runtime::RuntimeRoot;
        use std::sync::Arc;
        let parent = tempfile::tempdir().unwrap();
        let rt = Arc::new(RuntimeRoot::new_in(parent.path(), std::process::id()).unwrap());
        let svc = TerminalService::with_runtime_root(Arc::clone(&rt));
        let activity_id = ActivityId::new();
        let pane_id = PaneId::new();
        svc.spawn(
            pane_id,
            activity_id.clone(),
            SpawnOptions {
                cols: 80,
                rows: 24,
                shell: "/bin/sh".to_string(),
                cwd: None,
                window_id: None,
                session_id: None,
            },
        )
        .await
        .expect("spawn must succeed even with empty bin/");
        svc.kill(&activity_id).await.unwrap();
    }

    #[tokio::test]
    async fn inject_spawn_failure_makes_next_spawn_fail() {
        let svc = TerminalService::default();
        let aid = ActivityId::new();
        svc.inject_spawn_failure(aid.clone()).await;
        let result = svc
            .spawn(
                PaneId::new(),
                aid.clone(),
                SpawnOptions {
                    cols: 80,
                    rows: 24,
                    shell: "/bin/sh".into(),
                    cwd: None,
                    window_id: None,
                    session_id: None,
                },
            )
            .await;
        assert!(matches!(result, Err(TerminalError::Pty(_))));
        assert!(svc.subscriber_count(&aid).await.is_none());
    }
}
