//! Phase 2A VT WebSocket loop: hello frame + atomic subscribe + frame relay +
//! client input dispatch (resize / ack / input) with connection-local 1011.

use axum::extract::ws::{CloseFrame, Message, WebSocket};
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
    "mouse-vt200",
    "mouse-btn-event",
    "mouse-any-event",
    "mouse-sgr-1006",
    "focus-events",
    "app-cursor-keys",
];
const INPUT_CAPS: &[&str] = &["text-utf8", "key-vt-encoded"];

/// Inbound JSON control messages from the client.
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ClientControl {
    Resize {
        cols: u16,
        rows: u16,
    },
    Scroll {
        delta: i32,
    },
    ScrollToBottom,
    Ack {
        #[allow(dead_code, reason = "Phase 2A drains acks")]
        seq: u32,
    },
    ClientError {
        #[allow(dead_code, reason = "Phase 2A logs only")]
        category: String,
        #[serde(default)]
        #[allow(dead_code, reason = "Phase 2A logs only")]
        detail: Option<String>,
    },
}

/// Inbound msgpack binary messages from the client.
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ClientBinary {
    Input { data: serde_bytes::ByteBuf },
}

/// Decodes and dispatches an inbound text frame.
async fn handle_client_text(
    terminal: &TerminalService,
    aid: &ActivityId,
    text: &str,
) -> Result<(), String> {
    let ctrl: ClientControl =
        serde_json::from_str(text).map_err(|e| format!("json decode: {e}"))?;
    match ctrl {
        ClientControl::Resize { cols, rows } => {
            terminal
                .resize(aid, cols, rows)
                .await
                .map_err(|e| e.to_string())?;
        }
        ClientControl::Scroll { delta } => {
            terminal
                .scroll(aid, delta)
                .await
                .map_err(|e| e.to_string())?;
        }
        ClientControl::ScrollToBottom => {
            terminal
                .scroll_to_bottom(aid)
                .await
                .map_err(|e| e.to_string())?;
        }
        ClientControl::Ack { .. } => { /* Phase 2A drains */ }
        ClientControl::ClientError { .. } => { /* Phase 2A logs only */ }
    }
    Ok(())
}

/// Decodes and dispatches an inbound binary frame.
async fn handle_client_binary(
    terminal: &TerminalService,
    aid: &ActivityId,
    bytes: &[u8],
) -> Result<(), String> {
    let frame: ClientBinary =
        rmp_serde::from_slice(bytes).map_err(|e| format!("msgpack decode: {e}"))?;
    match frame {
        ClientBinary::Input { data } => {
            terminal
                .write(aid, &data)
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Sends an error text frame and then a Close(1011) frame on this connection only.
async fn send_error_and_close(
    tx: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    category: &str,
    detail: &str,
) {
    let err = serde_json::json!({
        "kind": "error",
        "category": category,
        "detail": detail,
    });
    let _ = tx.send(Message::Text(err.to_string().into())).await;
    let _ = tx
        .send(Message::Close(Some(CloseFrame {
            code: 1011,
            reason: category.to_string().into(),
        })))
        .await;
}

/// VT WebSocket loop: sends the hello frame, subscribes atomically, sends
/// the initial snapshot or replay deltas, then relays live frames until the
/// channel closes or the client disconnects. Client messages (resize, ack,
/// input) are dispatched concurrently; decode failures emit an error frame
/// and close the connection with WS code 1011.
pub(super) async fn vt_ws_loop(
    socket: WebSocket,
    terminal: TerminalService,
    aid: ActivityId,
    last_seq: Option<u32>,
) {
    let (mut tx, mut rx_ws) = socket.split();

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
        tokio::select! {
            srv = rx_frames.recv() => match srv {
                Ok(WireMessage::Binary { encoded, .. }) => {
                    if tx.send(Message::Binary(encoded)).await.is_err() {
                        break;
                    }
                }
                Ok(WireMessage::Text(s)) => {
                    if tx.send(Message::Text(s.into())).await.is_err() { break; }
                }
                Err(RecvError::Lagged(_)) => {
                    // NOTE: passing Some(0) forces SnapshotReason::Lagged because
                    // any non-empty ring's first seq is > 0. Passing None would
                    // yield Reconnect, which misrepresents the cause here. A
                    // dedicated API would be cleaner; revisit in Phase 2B.
                    match terminal.subscribe_frames(&aid, Some(0)).await {
                        Ok(FrameSubscription::FreshSnapshot { snapshot, rx }) => {
                            if tx.send(Message::Binary(snapshot)).await.is_err() { break; }
                            rx_frames = rx;
                        }
                        _ => break,
                    }
                }
                Err(RecvError::Closed) => break,
            },
            cli = rx_ws.next() => match cli {
                Some(Ok(Message::Text(s))) => {
                    if let Err(e) = handle_client_text(&terminal, &aid, s.as_str()).await {
                        send_error_and_close(&mut tx, "client_text_decode", &e).await;
                        break;
                    }
                }
                Some(Ok(Message::Binary(b))) => {
                    if let Err(e) = handle_client_binary(&terminal, &aid, &b).await {
                        send_error_and_close(&mut tx, "client_input_decode", &e).await;
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(_)) => break,
                _ => continue,
            }
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

    #[tokio::test]
    async fn client_text_resize_is_accepted() {
        use ozmux_terminal::vt::{FrameSnapshot, RenderFrame, SnapshotReason};

        let (addr, state, wid, pid, aid) =
            crate::handlers::windows::panes::activities::test_support::boot_server_full().await;
        let url = format!(
            "ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws?mode=vt&vt_version=vt-1"
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws.next().await.unwrap().unwrap(); // hello
        let _ = ws.next().await.unwrap().unwrap(); // initial snapshot

        // Drain any shell-startup deltas so the resize-induced snapshot is the
        // next thing the bridge has to emit (otherwise a queue of prompt
        // deltas can outlast the test deadline).
        let drain_deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        while tokio::time::Instant::now() < drain_deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(100), ws.next()).await {
                Ok(Some(Ok(_))) => continue,
                _ => break,
            }
        }

        // Send a resize JSON.
        let resize = serde_json::json!({ "kind": "resize", "cols": 80, "rows": 30 });
        use futures_util::SinkExt;
        ws.send(TtMessage::Text(resize.to_string().into()))
            .await
            .unwrap();

        // Expect a Binary Snapshot{Resize} to follow within 10s.
        let mut saw = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(500), ws.next()).await {
                Ok(Some(Ok(TtMessage::Binary(b)))) => {
                    let f: RenderFrame = rmp_serde::from_slice(&b).unwrap();
                    if let RenderFrame::Snapshot(FrameSnapshot { reason, .. }) = f
                        && matches!(reason, SnapshotReason::Resize)
                    {
                        saw = true;
                        break;
                    }
                }
                Ok(Some(Ok(TtMessage::Close(_)))) | Ok(None) => break,
                _ => continue,
            }
        }
        let _ = state;
        assert!(saw, "client resize must trigger Snapshot{{Resize}}");
        ws.close(None).await.ok();
    }

    #[tokio::test]
    async fn malformed_client_text_closes_with_1011() {
        let (addr, _state, wid, pid, aid) =
            crate::handlers::windows::panes::activities::test_support::boot_server_full().await;
        let url = format!(
            "ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws?mode=vt&vt_version=vt-1"
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws.next().await.unwrap().unwrap();
        let _ = ws.next().await.unwrap().unwrap();

        use futures_util::SinkExt;
        ws.send(TtMessage::Text("not json".into())).await.unwrap();

        // Expect a Close from the server within 2s with code 1011 (Error in tungstenite).
        let mut closed_1011 = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(200), ws.next()).await {
                Ok(Some(Ok(TtMessage::Close(Some(frame))))) => {
                    if frame.code
                        == tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Error
                    {
                        closed_1011 = true;
                        break;
                    }
                }
                Ok(None) => break,
                _ => continue,
            }
        }
        assert!(closed_1011, "expected Close(1011) after malformed input");
    }

    #[tokio::test]
    async fn end_to_end_echo_appears_in_delta_text() {
        use futures_util::SinkExt;
        use ozmux_terminal::vt::{FrameDelta, RenderFrame};

        let (addr, _state, wid, pid, aid) =
            crate::handlers::windows::panes::activities::test_support::boot_server_full().await;
        let url = format!(
            "ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws?mode=vt&vt_version=vt-1"
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws.next().await.unwrap().unwrap(); // hello
        let _ = ws.next().await.unwrap().unwrap(); // initial snapshot

        // Drain any shell-startup deltas so the echo response is the next emit.
        let drain_deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        while tokio::time::Instant::now() < drain_deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(100), ws.next()).await {
                Ok(Some(Ok(_))) => continue,
                _ => break,
            }
        }

        // Send a client input frame: msgpack-encoded {kind: "input", data: bytes}.
        let input_payload = rmp_serde::to_vec_named(&serde_json::json!({
            "kind": "input",
            "data": serde_bytes::Bytes::new(b"echo wire_loop\n"),
        }))
        .unwrap();
        ws.send(TtMessage::Binary(input_payload.into()))
            .await
            .unwrap();

        // Collect frames until a Snapshot or Delta contains "wire_loop".
        let mut saw = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(500), ws.next()).await {
                Ok(Some(Ok(TtMessage::Binary(b)))) => {
                    let f: RenderFrame = rmp_serde::from_slice(&b).unwrap();
                    let text: String = match f {
                        RenderFrame::Snapshot(s) => s
                            .rows_data
                            .iter()
                            .flat_map(|r| r.runs.iter().map(|run| run.text.clone()))
                            .collect(),
                        RenderFrame::Delta(FrameDelta { dirty_rows, .. }) => dirty_rows
                            .iter()
                            .flat_map(|d| d.runs.iter().map(|run| run.text.clone()))
                            .collect(),
                    };
                    if text.contains("wire_loop") {
                        saw = true;
                        break;
                    }
                }
                Ok(Some(Ok(TtMessage::Close(_)))) | Ok(None) => break,
                _ => continue,
            }
        }
        assert!(saw, "expected 'wire_loop' to appear in a server frame");
        ws.close(None).await.ok();
    }
}
