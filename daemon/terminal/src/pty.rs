use crate::{
    error::{PtyErrorBridge, TerminalError, TerminalResult},
    pty::pty_handle::{PtyHandle, ScrollbackBuffer},
    vt::frame::{RenderFrame, SnapshotReason, encode},
    vt::frame_builder::build_snapshot,
    vt::frame_ring::WireMessage,
};
use bytes::Bytes;
use ozmux_extension::runtime::RuntimeRoot;
use ozmux_multiplexer::{ActivityId, PaneId, SessionId, WindowId};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Read, sync::Arc};
use tokio::sync::{RwLock, RwLockReadGuard, broadcast};

pub(crate) mod pty_handle;
mod ring_buffer;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TerminalEvent {
    Data { buffer: Vec<u8> },
    Exit { code: Option<i32> },
}

/// Snapshot of the alacritty `TermDamage` state captured by the test-only
/// [`TerminalService::inspect_damage_and_reset`] helper. Used by Phase 1
/// PoC integration tests to characterize the damage API's observable
/// behavior in alacritty 0.26.
#[cfg(any(test, feature = "test-helpers"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DamageSnapshot {
    /// `Term::damage()` returned `TermDamage::Full` — the entire viewport
    /// is considered dirty.
    Full,
    /// `Term::damage()` returned `TermDamage::Partial(iter)`; `line_count`
    /// is the number of damaged lines yielded by the iterator.
    Partial { line_count: usize },
}

#[derive(Default, Clone)]
pub struct TerminalService {
    ptys: Arc<RwLock<HashMap<ActivityId, PtyHandle>>>,
    runtime_root: Option<Arc<RuntimeRoot>>,
}

pub struct SpawnOptions {
    pub cols: u16,
    pub rows: u16,
    pub shell: String,
    pub cwd: Option<String>,
    /// Owning Window id, surfaced to the spawned shell as `OZMUX_WINDOW_ID`.
    /// `None` only for callers that have no Window context (tests/legacy).
    pub window_id: Option<WindowId>,
    /// Owning Session id, surfaced to the spawned shell as `OZMUX_SESSION_ID`
    /// when present. Orphan Windows resolve to `None`.
    pub session_id: Option<SessionId>,
}

/// Outcome of subscribing to an activity's wire stream.
///
/// Callers render the snapshot or apply the replayed deltas, then consume
/// `rx` for all subsequent emissions without gaps.
pub enum FrameSubscription {
    /// Server emitted a fresh snapshot atomically with the subscription.
    /// Client should render the snapshot then consume `rx` for deltas.
    FreshSnapshot {
        /// Encoded MessagePack of the snapshot.
        snapshot: Bytes,
        /// Receiver for subsequent wire messages.
        rx: broadcast::Receiver<WireMessage>,
    },
    /// Server replayed buffered deltas covering `[last_seq+1, latest]`.
    /// Client applies each delta in order then consumes `rx` for further
    /// deltas.
    ResumeReplay {
        /// Buffered deltas in seq order.
        deltas: Vec<Bytes>,
        /// Receiver for subsequent wire messages.
        rx: broadcast::Receiver<WireMessage>,
    },
}

impl TerminalService {
    pub fn with_runtime_root(root: Arc<RuntimeRoot>) -> Self {
        Self {
            ptys: Arc::default(),
            runtime_root: Some(root),
        }
    }

    pub async fn spawn(
        &self,
        pane_id: PaneId,
        activity_id: ActivityId,
        opts: SpawnOptions,
    ) -> TerminalResult {
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

        let (vt_chunk_tx, vt_chunk_rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(128);

        spawn_pty_reader(
            reader,
            child,
            scrollback.clone(),
            event_sender.clone(),
            vt_chunk_tx.clone(),
        );

        let handle = PtyHandle::new(
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
        ptys.insert(activity_id, handle);
        Ok(())
    }

    pub async fn write(&self, activity: &ActivityId, data: &[u8]) -> TerminalResult {
        let handle = self.read(activity).await?;
        handle
            .writer
            .lock()
            .await
            .write_all(data)
            .map_err(|e| TerminalError::Pty(e.to_string()))?;
        Ok(())
    }

    pub async fn resize(&self, activity: &ActivityId, cols: u16, rows: u16) -> TerminalResult {
        let handle = self.read(activity).await?;

        // 1) Resize the alacritty Term first so the next bridge cycle sees Full damage.
        {
            let dim = crate::vt::bridge::dim_for(cols, rows);
            let mut state = handle.vt_state.lock().expect("vt_state poisoned");
            state.term.resize(dim);
        }

        // 2) Resize the PTY master (existing behavior).
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

        // 3) Wake the bridge so it observes Full damage and emits a snapshot.
        let _ = handle.vt_chunk_tx.try_send(bytes::Bytes::new());

        Ok(())
    }

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

        // Single critical section: snapshot (or replay) AND subscribe.
        let state = vt_state.lock().expect("vt_state poisoned");

        // Resume path: ring has the requested seq range available.
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
        let snap = build_snapshot(&state.term, snap_seq, reason);
        let encoded_vec =
            encode(&RenderFrame::Snapshot(snap)).expect("encode infallible for valid frame");
        let rx = state.wire_broadcast.subscribe();
        Ok(FrameSubscription::FreshSnapshot {
            snapshot: Bytes::from(encoded_vec),
            rx,
        })
    }

    /// Return the current broadcast subscriber count for an activity, or `None`
    /// if the activity has no PTY. Used in tests to verify task lifecycle.
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn subscriber_count(&self, activity: &ActivityId) -> Option<usize> {
        let guard = self.read(activity).await.ok()?;
        Some(guard.event_sender.receiver_count())
    }

    /// Test-only read of the VT Term grid: returns the first `cols` characters
    /// of the given `row` as a `String`. Returns `None` if the activity has no
    /// PTY. The VtState lock is short-held and dropped before returning.
    ///
    /// Intended exclusively for integration tests that need to assert the
    /// bridge task has applied PTY output to the in-memory `Term`. Production
    /// code should not depend on this surface.
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn inspect_row(
        &self,
        activity: &ActivityId,
        row: i32,
        cols: usize,
    ) -> Option<String> {
        use alacritty_terminal::index::{Column, Line};
        let guard = self.read(activity).await.ok()?;
        let vt_state = guard.vt_state.clone();
        drop(guard);
        let state = vt_state.lock().expect("vt_state lock poisoned");
        let term_row = &state.term.grid()[Line(row)];
        let slice = &term_row[Column(0)..Column(cols)];
        Some(slice.iter().map(|cell| cell.c).collect())
    }

    /// Test-only probe of the alacritty damage tracker: reads
    /// `Term::damage()` into a [`DamageSnapshot`] and then calls
    /// `Term::reset_damage()` under the same VT lock. Returns `None` if
    /// the activity has no PTY.
    ///
    /// Used by Phase 1 PoC tests to characterize the API: whether
    /// `Full` is sticky across resets, how many lines `Partial` reports
    /// after typical shell output, etc.
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn inspect_damage_and_reset(&self, activity: &ActivityId) -> Option<DamageSnapshot> {
        use alacritty_terminal::term::TermDamage;
        let guard = self.read(activity).await.ok()?;
        let vt_state = guard.vt_state.clone();
        drop(guard);
        let mut state = vt_state.lock().expect("vt_state lock poisoned");
        let snapshot = match state.term.damage() {
            TermDamage::Full => DamageSnapshot::Full,
            TermDamage::Partial(iter) => {
                let line_count = iter.count();
                DamageSnapshot::Partial { line_count }
            }
        };
        state.term.reset_damage();
        Some(snapshot)
    }

    /// Test-only: raw subscription to the wire broadcast (no atomicity guarantee).
    /// Production paths use `subscribe_frames` (Task 13).
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn subscribe_wire_broadcast(
        &self,
        activity: &ActivityId,
    ) -> Option<tokio::sync::broadcast::Receiver<crate::vt::frame_ring::WireMessage>> {
        let guard = self.read(activity).await.ok()?;
        let vt_state = guard.vt_state.clone();
        drop(guard);
        let state = vt_state.lock().expect("vt_state poisoned");
        Some(state.wire_broadcast.subscribe())
    }

    /// Test-only probe of `TermMode::ALT_SCREEN`. Returns `Some(true)`
    /// when the terminal is currently on the alternate screen buffer,
    /// `Some(false)` when on the primary buffer, and `None` if the
    /// activity has no PTY. Used by Phase 1 PoC tests to verify which
    /// DEC private mode escapes (`?47` / `?1047` / `?1049`) toggle the
    /// alt-screen flag in alacritty 0.26.
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn inspect_alt_screen(&self, activity: &ActivityId) -> Option<bool> {
        use alacritty_terminal::term::TermMode;
        let guard = self.read(activity).await.ok()?;
        let vt_state = guard.vt_state.clone();
        drop(guard);
        let state = vt_state.lock().expect("vt_state lock poisoned");
        Some(state.term.mode().contains(TermMode::ALT_SCREEN))
    }

    #[inline]
    async fn read(
        &self,
        activity_id: &ActivityId,
    ) -> TerminalResult<RwLockReadGuard<'_, PtyHandle>> {
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

/// Spawn a dedicated OS thread for blocking PTY reads, plus a tokio bridge
/// task that pushes data to the scrollback and broadcasts under the same
/// lock (race-free with snapshot_and_subscribe).
///
/// The OS thread is preferred over `tokio::spawn` here because `read()` is
/// blocking and would otherwise occupy a tokio worker.
fn spawn_pty_reader(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    scrollback: ScrollbackBuffer,
    event_sender: broadcast::Sender<TerminalEvent>,
    vt_chunk_tx: tokio::sync::mpsc::Sender<bytes::Bytes>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let exit_event_sender = event_sender.clone();

    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        let _ = exit_event_sender.send(TerminalEvent::Exit { code });
    });

    // NOTE: VT fan-out runs before push_and_broadcast so the raw path is
    // never stalled by VT consumption pressure; try_send drops silently when
    // the VT bridge can't keep up, and the raw scrollback path remains
    // source of truth.
    tokio::spawn(async move {
        while let Some(chunk) = rx.recv().await {
            let _ = vt_chunk_tx.try_send(bytes::Bytes::copy_from_slice(&chunk));
            scrollback.push_and_broadcast(&event_sender, chunk).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
