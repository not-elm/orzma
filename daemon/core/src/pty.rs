use crate::{
    error::{OzmuxError, OzmuxResult, PtyErrorBridge},
    pty::pty_handle::{PtyHandle, ScrollbackBuffer},
    session::activity::ActivityId,
};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Read, sync::Arc};
use tokio::sync::{RwLock, RwLockReadGuard, broadcast};

pub mod pty_handle;
mod ring_buffer;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TerminalEvent {
    Data { buffer: Vec<u8> },
    Exit { code: Option<i32> },
}

#[derive(Default, Clone)]
pub struct TerminalService {
    pub ptys: Arc<RwLock<HashMap<ActivityId, PtyHandle>>>,
}

pub struct SpawnOptions {
    pub cols: u16,
    pub rows: u16,
    pub shell: String,
    pub cwd: Option<String>,
}

impl TerminalService {
    pub async fn spawn(&self, activity_id: ActivityId, opts: SpawnOptions) -> OzmuxResult {
        if self.ptys.read().await.contains_key(&activity_id) {
            return Ok(());
        }
        let pty_pair = native_pty_system()
            .openpty(PtySize {
                rows: opts.rows,
                cols: opts.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .to_ozmux_result()?;

        let mut cmd = CommandBuilder::new(&opts.shell);
        if let Some(cwd) = &opts.cwd {
            cmd.cwd(cwd);
        }
        let child = pty_pair.slave.spawn_command(cmd).to_ozmux_result()?;
        let killer = child.clone_killer();
        // Drop slave fd in this process so EOF propagates when shell exits.
        drop(pty_pair.slave);

        let reader = pty_pair.master.try_clone_reader().to_ozmux_result()?;
        let writer = pty_pair.master.take_writer().to_ozmux_result()?;
        let scrollback = ScrollbackBuffer::new();
        let (event_sender, _) = broadcast::channel(1024);

        spawn_pty_thread(reader, child, scrollback.clone(), event_sender.clone());

        let handle = PtyHandle::new(pty_pair.master, writer, event_sender, killer, scrollback);
        self.ptys.write().await.insert(activity_id, handle);
        Ok(())
    }

    pub async fn write(&self, activity: &ActivityId, data: &[u8]) -> OzmuxResult {
        let handle = self.read(activity).await?;
        handle
            .writer
            .lock()
            .await
            .write_all(data)
            .map_err(|e| OzmuxError::Pty(e.to_string()))?;
        Ok(())
    }

    pub async fn resize(&self, activity: &ActivityId, cols: u16, rows: u16) -> OzmuxResult {
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
            .to_ozmux_result()?;
        Ok(())
    }

    pub async fn kill(&self, activity: &ActivityId) -> OzmuxResult {
        if let Some(h) = self.ptys.write().await.remove(activity) {
            // Drop the handle; reader thread will exit when reader EOFs.
            drop(h);
        }
        Ok(())
    }

    pub async fn snapshot_and_subscribe(
        &self,
        activity: &ActivityId,
    ) -> OzmuxResult<(Vec<u8>, broadcast::Receiver<TerminalEvent>)> {
        let handle = self.read(activity).await?;
        Ok(handle.snapshot_and_subscribe().await)
    }

    #[inline]
    async fn read(&self, activity_id: &ActivityId) -> OzmuxResult<RwLockReadGuard<'_, PtyHandle>> {
        let guard = self.ptys.read().await;
        RwLockReadGuard::try_map(guard, |ptys| ptys.get(activity_id))
            .map_err(|_| OzmuxError::ActivityNotFound(activity_id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn snapshot_and_subscribe_returns_err_for_unknown_activity() {
        let svc = TerminalService::default();
        let id = ActivityId::new();
        let result = svc.snapshot_and_subscribe(&id).await;
        assert!(matches!(result, Err(OzmuxError::ActivityNotFound(ref got)) if got == &id));
    }

    #[tokio::test]
    async fn spawn_then_snapshot_and_subscribe_succeeds() {
        let svc = TerminalService::default();
        let id = ActivityId::new();
        svc.spawn(id.clone(), SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/sh".to_string(),
            cwd: None,
        }).await.unwrap();

        let (snap, _rx) = svc.snapshot_and_subscribe(&id).await.unwrap();
        // snapshot may be empty depending on shell startup speed; just confirm Ok.
        let _ = snap;

        // Cleanup
        svc.kill(&id).await.unwrap();
    }
}

fn spawn_pty_thread(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    scrollback: ScrollbackBuffer,
    event_tx: broadcast::Sender<TerminalEvent>,
) {
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    scrollback.push(&buf[..n]).await;
                    let _ = event_tx.send(TerminalEvent::Data {
                        buffer: buf[..n].to_vec(),
                    });
                }
                Err(_) => break,
            }
        }
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        let _ = event_tx.send(TerminalEvent::Exit { code });
    });
}
