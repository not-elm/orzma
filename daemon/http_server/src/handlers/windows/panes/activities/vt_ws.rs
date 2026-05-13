//! Phase 2A VT WebSocket loop: hello frame + atomic subscribe + frame relay.

use axum::extract::ws::WebSocket;
use ozmux_multiplexer::ActivityId;
use ozmux_terminal::TerminalService;

pub(super) async fn vt_ws_loop(
    _socket: WebSocket,
    _terminal: TerminalService,
    _aid: ActivityId,
    _last_seq: Option<u32>,
) {
    // Task 16/17/18/19 fill this in.
}
