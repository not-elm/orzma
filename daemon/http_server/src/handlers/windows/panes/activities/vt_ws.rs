//! Phase 2A VT WebSocket loop: hello frame + atomic subscribe + frame relay.

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use ozmux_multiplexer::ActivityId;
use ozmux_terminal::vt::WireMessage;
use ozmux_terminal::{FrameSubscription, TerminalService};
use tokio::sync::broadcast::error::RecvError;

const ESCAPE_CAPS: &[&str] = &[
    "sgr",
    "cup",
    "ed",
    "el",
    "decset",
    "decrst",
    "alt-screen-1049",
    "bracketed-paste",
];
const INPUT_CAPS: &[&str] = &["text-utf8", "key-vt-encoded"];

/// VT WebSocket loop: sends the hello frame, subscribes atomically, sends
/// the initial snapshot or replay deltas, then relays live frames until the
/// channel closes or the client disconnects.
pub(super) async fn vt_ws_loop(
    socket: WebSocket,
    terminal: TerminalService,
    aid: ActivityId,
    last_seq: Option<u32>,
) {
    let (mut tx, _rx_ws) = socket.split();

    let geom = match terminal.read_geometry(&aid).await {
        Ok(g) => g,
        Err(_) => return,
    };

    let hello = serde_json::json!({
        "kind": "hello",
        "seq": 0,
        "cols": geom.cols,
        "rows": geom.rows,
        "cursor": geom.cursor,
        "escape_caps": ESCAPE_CAPS,
        "input_caps": INPUT_CAPS,
    });
    if tx
        .send(Message::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    let mut rx_frames = match terminal.subscribe_frames(&aid, last_seq).await {
        Ok(FrameSubscription::FreshSnapshot { snapshot, rx }) => {
            if tx.send(Message::Binary(snapshot)).await.is_err() {
                return;
            }
            rx
        }
        Ok(FrameSubscription::ResumeReplay { deltas, rx }) => {
            for d in deltas {
                if tx.send(Message::Binary(d)).await.is_err() {
                    return;
                }
            }
            rx
        }
        Err(_) => return,
    };

    loop {
        match rx_frames.recv().await {
            Ok(WireMessage::Binary { encoded, .. }) => {
                if tx.send(Message::Binary(encoded)).await.is_err() {
                    break;
                }
            }
            Ok(WireMessage::Text(s)) => {
                if tx.send(Message::Text(s.into())).await.is_err() {
                    break;
                }
            }
            Err(RecvError::Lagged(_)) => {
                // NOTE: We pass Some(0) so subscribe_frames sees a last_seq
                // that is guaranteed to be before any ring entry, forcing a
                // FreshSnapshot with SnapshotReason::Lagged. Passing None
                // would instead yield SnapshotReason::Reconnect, which is
                // semantically wrong here. This is a Phase 2A expedient;
                // a dedicated API would be cleaner.
                match terminal.subscribe_frames(&aid, Some(0)).await {
                    Ok(FrameSubscription::FreshSnapshot { snapshot, rx }) => {
                        if tx.send(Message::Binary(snapshot)).await.is_err() {
                            break;
                        }
                        rx_frames = rx;
                    }
                    _ => break,
                }
            }
            Err(RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message as TtMessage;

    #[tokio::test]
    async fn hello_is_first_frame_with_required_fields() {
        let (addr, _state, wid, pid, aid) =
            crate::handlers::windows::panes::activities::test_support::boot_server_full().await;
        let url = format!(
            "ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws?mode=vt&vt_version=vt-1"
        );
        let (mut ws, _resp) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let msg = ws.next().await.unwrap().unwrap();
        match msg {
            TtMessage::Text(s) => {
                let v: serde_json::Value = serde_json::from_str(&s).unwrap();
                assert_eq!(v["kind"], "hello");
                assert_eq!(v["seq"], 0);
                assert!(v["cols"].is_number());
                assert!(v["rows"].is_number());
                assert!(v["cursor"].is_object());
                assert!(v["escape_caps"].is_array());
                assert!(v["input_caps"].is_array());
            }
            other => panic!("expected Text(hello), got {other:?}"),
        }
        ws.close(None).await.ok();
    }

    #[tokio::test]
    async fn snapshot_arrives_after_hello() {
        let (addr, _state, wid, pid, aid) =
            crate::handlers::windows::panes::activities::test_support::boot_server_full().await;
        let url = format!(
            "ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws?mode=vt&vt_version=vt-1"
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        // Skip hello.
        let _ = ws.next().await.unwrap().unwrap();
        // Next binary frame is the initial snapshot.
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match msg {
            TtMessage::Binary(b) => {
                let _frame: ozmux_terminal::vt::RenderFrame =
                    rmp_serde::from_slice(&b).expect("decode");
            }
            other => panic!("expected Binary snapshot, got {other:?}"),
        }
        ws.close(None).await.ok();
    }

    #[tokio::test]
    async fn more_frames_flow_after_initial_snapshot() {
        let (addr, state, wid, pid, aid) =
            crate::handlers::windows::panes::activities::test_support::boot_server_full().await;
        let url = format!(
            "ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws?mode=vt&vt_version=vt-1"
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        // Skip hello + initial snapshot.
        let _ = ws.next().await.unwrap().unwrap();
        let _ = ws.next().await.unwrap().unwrap();

        // Drive shell to produce more frames.
        state
            .terminal
            .write(&aid, b"echo loop_check\n")
            .await
            .unwrap();

        let mut got_more = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(200), ws.next()).await {
                Ok(Some(Ok(TtMessage::Binary(_)))) => {
                    got_more = true;
                    break;
                }
                Ok(Some(Ok(TtMessage::Text(_)))) => {
                    got_more = true;
                    break;
                }
                _ => continue,
            }
        }
        assert!(
            got_more,
            "more deltas/snapshots should flow after subscribe"
        );
        ws.close(None).await.ok();
    }
}
