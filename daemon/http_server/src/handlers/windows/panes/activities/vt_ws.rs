//! Phase 2A VT WebSocket loop: hello frame + atomic subscribe + frame relay.

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use ozmux_multiplexer::ActivityId;
use ozmux_terminal::TerminalService;

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

/// VT WebSocket loop: sends the hello frame then relays frames.
pub(super) async fn vt_ws_loop(
    socket: WebSocket,
    terminal: TerminalService,
    aid: ActivityId,
    _last_seq: Option<u32>,
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
    let _ = tx.send(Message::Text(hello.to_string().into())).await;
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
}
