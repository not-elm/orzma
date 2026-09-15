//! A pane's PTY input mailbox: a byte-capped FIFO that takes writes without
//! blocking, drained into the PTY by a dedicated writer thread.

use crate::error::{OrzmaTtyError, OrzmaTtyResult};
use std::collections::VecDeque;
use std::io::{Error as IoError, Write};
use std::mem;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

/// A pane's PTY input queue, drained by its own writer thread.
///
/// Each queued write is handed to the writer as one buffer, in the order
/// the writes were queued. The writer thread is never joined.
pub(crate) struct WriteQueue {
    shared: Arc<Shared>,
}

impl WriteQueue {
    /// Starts a writer thread named `orzma-pty-writer` that drains queued
    /// writes into `writer`, accepting at most `capacity` unwritten bytes.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmaTtyError::PtyWriterThread`] when the OS refuses to
    /// start the thread.
    pub fn spawn(writer: Box<dyn Write + Send>, capacity: usize) -> OrzmaTtyResult<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(QueueState {
                pending: VecDeque::new(),
                pending_bytes: 0,
                capacity,
                status: QueueStatus::Open,
                dropped_in_episode: 0,
            }),
            work: Condvar::new(),
            settled: Condvar::new(),
        });
        let thread_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("orzma-pty-writer".to_string())
            .spawn(move || drain(writer, &thread_shared))
            .map_err(OrzmaTtyError::PtyWriterThread)?;
        Ok(Self { shared })
    }

    /// Queues `bytes` without blocking; `Ok` means the bytes were queued,
    /// not that they were written. An empty buffer queues nothing.
    ///
    /// # Errors
    ///
    /// - [`OrzmaTtyError::PtyWriteQueueFull`] when `bytes` would push the
    ///   unwritten bytes past the capacity; nothing is queued. The count
    ///   restarts at 1 when the queue has been empty since the last
    ///   rejection.
    /// - [`OrzmaTtyError::PtyWrite`] once, after the writer's OS write
    ///   failed.
    /// - [`OrzmaTtyError::PtyWriterClosed`] after that, and once the queue
    ///   is closed.
    pub fn enqueue(&self, bytes: Vec<u8>) -> OrzmaTtyResult {
        let mut state = lock(&self.shared.state);
        match &mut state.status {
            QueueStatus::Closed => return Err(OrzmaTtyError::PtyWriterClosed),
            QueueStatus::Failed(error) => {
                return Err(error
                    .take()
                    .map_or(OrzmaTtyError::PtyWriterClosed, OrzmaTtyError::PtyWrite));
            }
            QueueStatus::Open => {}
        }
        if bytes.is_empty() {
            return Ok(());
        }
        if state.pending_bytes == 0 {
            state.dropped_in_episode = 0;
        }
        if state.pending_bytes.saturating_add(bytes.len()) > state.capacity {
            state.dropped_in_episode = state.dropped_in_episode.saturating_add(1);
            return Err(OrzmaTtyError::PtyWriteQueueFull {
                dropped_in_episode: state.dropped_in_episode,
            });
        }
        state.pending_bytes += bytes.len();
        state.pending.push_back(bytes);
        drop(state);
        self.shared.work.notify_one();
        Ok(())
    }

    /// Blocks until every queued write has been written, or until the
    /// queue stops being open.
    ///
    /// Never returns while the writer is blocked in a write that does not
    /// complete.
    #[cfg(any(test, feature = "test-support"))]
    pub fn settle(&self) {
        let mut state = lock(&self.shared.state);
        while matches!(state.status, QueueStatus::Open) && state.pending_bytes > 0 {
            state = self
                .shared
                .settled
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Stops accepting writes, replacing any writer failure not yet
    /// reported. The writer thread discards what is still queued once any
    /// write in flight returns, then drops the writer and exits.
    fn close(&self) {
        let mut state = lock(&self.shared.state);
        state.status = QueueStatus::Closed;
        drop(state);
        self.shared.work.notify_all();
        self.shared.settled.notify_all();
    }
}

impl Drop for WriteQueue {
    fn drop(&mut self) {
        // NOTE: the writer thread is never joined, because joining would
        // freeze the backend thread dropping the pane. The thread can stay
        // blocked in a PTY write, including the end-of-file write that
        // dropping portable-pty's Unix writer makes, until the application
        // reads its input again or the PTY's slave side closes; killing the
        // child guarantees neither. While it is blocked, it keeps the
        // unwritten bytes, up to the queue's capacity, alive after the pane
        // is gone.
        self.close();
    }
}

/// The state the owner and the writer thread share.
struct Shared {
    state: Mutex<QueueState>,
    /// Wakes the writer thread when a write is queued or the queue closes.
    work: Condvar,
    /// Wakes `settle` when the queue drains or stops being open.
    settled: Condvar,
}

/// Everything guarded by the queue's single lock.
struct QueueState {
    /// Queued writes, each a whole logical write.
    pending: VecDeque<Vec<u8>>,
    /// Bytes queued plus the bytes of the write in flight.
    pending_bytes: usize,
    capacity: usize,
    status: QueueStatus,
    /// Writes rejected since the queue was last empty.
    dropped_in_episode: u64,
}

/// Where a queue stands in its lifecycle.
enum QueueStatus {
    /// The writer thread is draining the queue.
    Open,
    /// The owner closed the queue.
    Closed,
    /// The writer's OS write failed; the error is handed out once.
    Failed(Option<IoError>),
}

/// Runs the writer thread: writes each queued buffer in order until the
/// queue closes or a write fails.
fn drain(mut writer: Box<dyn Write + Send>, shared: &Shared) {
    while let Some(bytes) = next_write(shared) {
        let result = writer.write_all(&bytes);
        let mut state = lock(&shared.state);
        match result {
            Ok(()) => {
                state.pending_bytes = state.pending_bytes.saturating_sub(bytes.len());
                if state.pending_bytes == 0 {
                    state.dropped_in_episode = 0;
                    drop(state);
                    shared.settled.notify_all();
                }
            }
            Err(error) => {
                if matches!(state.status, QueueStatus::Open) {
                    state.status = QueueStatus::Failed(Some(error));
                }
            }
        }
    }
}

/// Waits for the next queued write, keeping its bytes counted as in
/// flight; `None` once the queue is no longer open, after discarding what
/// is left.
fn next_write(shared: &Shared) -> Option<Vec<u8>> {
    let mut state = lock(&shared.state);
    loop {
        if !matches!(state.status, QueueStatus::Open) {
            let discarded = mem::take(&mut state.pending);
            state.pending_bytes = 0;
            drop(state);
            shared.settled.notify_all();
            drop(discarded);
            return None;
        }
        if let Some(bytes) = state.pending.pop_front() {
            return Some(bytes);
        }
        state = shared
            .work
            .wait(state)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// Locks the queue state, recovering it from a poisoned lock.
fn lock(state: &Mutex<QueueState>) -> MutexGuard<'_, QueueState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{BlockingSink, CaptureSink, FailingSink};
    use crossbeam_channel::bounded;
    use std::time::{Duration, Instant};

    /// Polls `condition` until it holds, failing after ten seconds.
    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "the condition never held");
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// The drop count a `PtyWriteQueueFull` result carries, or `None` for
    /// any other result.
    fn queue_full(result: OrzmaTtyResult) -> Option<u64> {
        match result {
            Err(OrzmaTtyError::PtyWriteQueueFull { dropped_in_episode }) => {
                Some(dropped_in_episode)
            }
            _ => None,
        }
    }

    /// Asserts that queued writes reach the writer whole and in the order
    /// they were queued.
    ///
    /// Case: the user types a key, pastes a line, and types another key in
    /// quick succession.
    #[test]
    fn queued_writes_arrive_whole_and_in_order() {
        let sink = CaptureSink::default();
        let queue = WriteQueue::spawn(Box::new(sink.clone()), 64).expect("writer thread");
        queue.enqueue(b"a".to_vec()).expect("queued");
        queue
            .enqueue(b"\x1b[200~line\x1b[201~".to_vec())
            .expect("queued");
        queue.enqueue(b"b".to_vec()).expect("queued");
        queue.settle();
        assert_eq!(sink.contents(), b"a\x1b[200~line\x1b[201~b");
    }

    /// Asserts that a write which would exceed the cap is rejected whole,
    /// that rejections within one stuck episode count up, and that the
    /// count restarts once the queue drains.
    ///
    /// Case: the user keeps typing into an application that stopped
    /// reading stdin, and the application later resumes reading.
    #[test]
    fn rejections_count_up_within_an_episode_and_restart_after_the_queue_drains() {
        let sink = BlockingSink::default();
        let queue = WriteQueue::spawn(Box::new(sink.clone()), 4).expect("writer thread");
        queue.enqueue(b"abcd".to_vec()).expect("fills the cap");
        assert_eq!(queue_full(queue.enqueue(b"e".to_vec())), Some(1));
        assert_eq!(queue_full(queue.enqueue(b"f".to_vec())), Some(2));
        sink.release();
        queue.settle();
        assert_eq!(lock(&queue.shared.state).dropped_in_episode, 0);
    }

    /// Asserts that a single write larger than the cap is rejected even
    /// into an empty queue, each such rejection starting its own episode.
    ///
    /// Case: the user pastes a file larger than the PTY input cap into an
    /// idle shell, twice.
    #[test]
    fn a_write_larger_than_the_cap_is_rejected_into_an_empty_queue() {
        let sink = CaptureSink::default();
        let queue = WriteQueue::spawn(Box::new(sink.clone()), 4).expect("writer thread");
        assert_eq!(queue_full(queue.enqueue(b"abcde".to_vec())), Some(1));
        assert_eq!(queue_full(queue.enqueue(b"abcde".to_vec())), Some(1));
        queue.settle();
        assert_eq!(sink.contents(), b"");
    }

    /// Asserts that a write accepted into an empty queue starts a new
    /// episode, so the next rejection reports a count of 1 again.
    ///
    /// Case: the user pastes a file larger than the PTY input cap, then pastes
    /// a smaller file that is still being written when a third paste arrives.
    #[test]
    fn a_write_accepted_into_an_empty_queue_restarts_the_count() {
        let sink = BlockingSink::default();
        let queue = WriteQueue::spawn(Box::new(sink.clone()), 4).expect("writer thread");
        assert_eq!(queue_full(queue.enqueue(b"abcde".to_vec())), Some(1));
        queue
            .enqueue(b"abc".to_vec())
            .expect("fits into the empty queue");
        assert_eq!(queue_full(queue.enqueue(b"de".to_vec())), Some(1));
        sink.release();
        queue.settle();
    }

    /// Asserts that a write the writer thread is still performing counts
    /// toward the cap.
    ///
    /// Case: an application stops reading while the first large paste is
    /// still being written to its PTY.
    #[test]
    fn a_write_in_flight_counts_toward_the_cap() {
        let sink = BlockingSink::default();
        let queue = WriteQueue::spawn(Box::new(sink.clone()), 4).expect("writer thread");
        queue.enqueue(b"abc".to_vec()).expect("queued");
        wait_until(|| {
            let state = lock(&queue.shared.state);
            state.pending.is_empty() && state.pending_bytes == 3
        });
        assert_eq!(queue_full(queue.enqueue(b"de".to_vec())), Some(1));
        queue
            .enqueue(b"d".to_vec())
            .expect("fits beside the write in flight");
        sink.release();
        queue.settle();
    }

    /// Asserts that after the writer's OS write fails, the next queued
    /// write reports that failure once and later writes report a closed
    /// writer.
    ///
    /// Case: the shell exits and its PTY starts refusing writes while the
    /// user is still typing.
    #[test]
    fn a_failed_write_is_reported_once_then_the_writer_reads_as_closed() {
        let queue = WriteQueue::spawn(Box::new(FailingSink), 64).expect("writer thread");
        queue
            .enqueue(b"a".to_vec())
            .expect("queued before the failure is known");
        queue.settle();
        assert!(matches!(
            queue.enqueue(b"b".to_vec()),
            Err(OrzmaTtyError::PtyWrite(_))
        ));
        assert!(matches!(
            queue.enqueue(b"c".to_vec()),
            Err(OrzmaTtyError::PtyWriterClosed)
        ));
    }

    /// Asserts that a closed queue refuses writes as a closed writer.
    ///
    /// Case: a keystroke arrives for a pane the backend is tearing down.
    #[test]
    fn a_closed_queue_refuses_writes() {
        let queue = WriteQueue::spawn(Box::new(CaptureSink::default()), 64).expect("writer thread");
        queue.close();
        assert!(matches!(
            queue.enqueue(b"a".to_vec()),
            Err(OrzmaTtyError::PtyWriterClosed)
        ));
    }

    /// Asserts that dropping the queue returns promptly while its writer
    /// thread is blocked in a write.
    ///
    /// Case: the user kills a pane whose application stopped reading stdin
    /// in the middle of a paste.
    #[test]
    fn dropping_the_queue_does_not_wait_for_a_blocked_writer() {
        let sink = BlockingSink::default();
        let queue = WriteQueue::spawn(Box::new(sink.clone()), 64).expect("writer thread");
        queue.enqueue(b"stuck".to_vec()).expect("queued");
        wait_until(|| lock(&queue.shared.state).pending.is_empty());
        let (done_tx, done_rx) = bounded(1);
        thread::spawn(move || {
            drop(queue);
            let _ = done_tx.send(());
        });
        let dropped = done_rx.recv_timeout(Duration::from_secs(5)).is_ok();
        sink.release();
        assert!(dropped, "dropping the queue waited for the blocked writer");
    }
}
