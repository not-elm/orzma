//! Background task: re-broadcasts a window's `WindowView` when one of its
//! terminal titles changes, coalescing bursts within a short debounce window.

use crate::AppState;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

/// Trailing debounce window. Terminal titles can change per redraw; this
/// bounds the `WindowView` re-broadcast rate so a title storm cannot push
/// a slow events-WS subscriber into `Lagged`.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Runs until the title-change channel closes (daemon shutdown).
pub(crate) async fn run(state: AppState) {
    let mut rx = state.titles.subscribe();
    loop {
        let first = match rx.recv().await {
            Ok(wid) => wid,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return,
        };
        let mut pending = HashSet::from([first]);
        let deadline = Instant::now() + DEBOUNCE;
        let mut closed = false;
        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(wid)) => {
                    pending.insert(wid);
                }
                Ok(Err(RecvError::Lagged(_))) => continue,
                Ok(Err(RecvError::Closed)) => {
                    closed = true;
                    break;
                }
                Err(_elapsed) => break,
            }
        }
        for wid in pending.drain() {
            state.publish_window_layout(&wid).await;
        }
        if closed {
            return;
        }
    }
}
