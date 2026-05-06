use crate::{
    error::{PtyErrorBridge, TerminalError, TerminalResult},
    pty::pty_handle::{PtyHandle, ScrollbackBuffer},
};
use ozmux_session::activity::ActivityId;
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
}

pub struct SpawnOptions {
    pub cols: u16,
    pub rows: u16,
    pub shell: String,
    pub cwd: Option<String>,
}

impl TerminalService {
    pub async fn spawn(&self, activity_id: ActivityId, opts: SpawnOptions) -> TerminalResult {
        // Hold the write lock for the entire spawn-then-insert so concurrent
        // callers with the same ActivityId cannot both pass the existence
        // check and end up double-spawning a PTY (which would leak the
        // overwritten one's child process and reader thread).
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
        let child = pty_pair.slave.spawn_command(cmd).to_terminal_result()?;
        let killer = child.clone_killer();
        // Drop slave fd in this process so EOF propagates when shell exits.
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
        if let Some(h) = self.ptys.write().await.remove(activity) {
            // Drop the handle; reader thread will exit when reader EOFs.
            drop(h);
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

    #[inline]
    async fn read(&self, activity_id: &ActivityId) -> TerminalResult<RwLockReadGuard<'_, PtyHandle>> {
        let guard = self.ptys.read().await;
        RwLockReadGuard::try_map(guard, |ptys| ptys.get(activity_id))
            .map_err(|_| TerminalError::ActivityNotFound(activity_id.clone()))
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
        svc.spawn(
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
    async fn pty_output_is_broadcast_to_subscribers() {
        let svc = TerminalService::default();
        let id = ActivityId::new();
        svc.spawn(
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
}
