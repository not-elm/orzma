use crate::{
    error::{PtyErrorBridge, TerminalError, TerminalResult},
    pty::pty_handle::{PtyHandle, ScrollbackBuffer},
};
use ozmux_extension::runtime::RuntimeRoot;
use ozmux_multiplexer::{activity::ActivityId, pane::PaneId};
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

        spawn_pty_reader(reader, child, scrollback.clone(), event_sender.clone());

        let handle = PtyHandle::new(pty_pair.master, writer, event_sender, killer, scrollback);
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

    /// Return the current broadcast subscriber count for an activity, or `None`
    /// if the activity has no PTY. Used in tests to verify task lifecycle.
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn subscriber_count(&self, activity: &ActivityId) -> Option<usize> {
        let guard = self.read(activity).await.ok()?;
        Some(guard.event_sender.receiver_count())
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
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let exit_event_sender = event_sender.clone();

    // 1) blocking read 専用の OS スレッド
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break; // bridge task が落ちた / drop された
                    }
                }
                Err(_) => break,
            }
        }
        // child.wait() もブロッキング → 同じ OS スレッドで実行
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        let _ = exit_event_sender.send(TerminalEvent::Exit { code });
        // tx は drop され、bridge task の rx.recv() が None を返して終わる
    });

    // 2) bridge task: mpsc → scrollback + broadcast を「同じロック内」で
    tokio::spawn(async move {
        while let Some(chunk) = rx.recv().await {
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
            },
        )
        .await
        .expect("spawn must succeed even with empty bin/");
        svc.kill(&activity_id).await.unwrap();
    }
}
