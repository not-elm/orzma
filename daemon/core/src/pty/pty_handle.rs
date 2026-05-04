use crate::{
    error::{OzmuxResult, PtyErrorBridge},
    pty::{TerminalEvent, ring_buffer::RingBuffer},
};
use portable_pty::{ChildKiller, MasterPty, PtyPair, PtySize};
use std::{io::Write, num::NonZero, sync::Arc};
use tokio::sync::{
    Mutex,
    broadcast::{Receiver, Sender},
};

#[derive(Clone, Debug)]
pub struct ScrollbackBuffer(Arc<Mutex<RingBuffer>>);

impl ScrollbackBuffer {
    const SCROLLBACK_BYTES: usize = 256 * 1024;
    pub fn new() -> Self {
        let capacity = NonZero::new(Self::SCROLLBACK_BYTES).unwrap();
        Self(Arc::new(Mutex::new(RingBuffer::with_capacity(capacity))))
    }

    #[inline]
    pub async fn push(&self, data: &[u8]) {
        self.0.lock().await.push(data);
    }

    #[inline]
    pub async fn snapshot(&self) -> Vec<u8> {
        self.0.lock().await.snapshot()
    }
}

pub(super) struct PtyHandle {
    pub master: Mutex<Box<dyn MasterPty + Send>>,
    pub writer: Mutex<Box<dyn Write + Send>>,
    pub scrollback: ScrollbackBuffer,
    pub event_sender: Sender<TerminalEvent>,
    _child_killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}

impl PtyHandle {
    pub fn new(
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        event_sender: Sender<TerminalEvent>,
        child_killer: Box<dyn ChildKiller + Send + Sync>,
        scrollback: ScrollbackBuffer,
    ) -> Self {
        Self {
            scrollback,
            event_sender,
            writer: Mutex::new(writer),
            master: Mutex::new(master),
            _child_killer: child_killer,
        }
    }

    #[inline]
    pub fn subscribe(&self) -> Receiver<TerminalEvent> {
        self.event_sender.subscribe()
    }
}
