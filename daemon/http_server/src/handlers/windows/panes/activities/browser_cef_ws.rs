//! WebSocket handler for cef-backed BrowserActivities. Parallel to
//! `browser_ws.rs`; activates when the frontend connects to
//! `/.../{activity_id}/browser_cef/ws` (gated by `?cef=1` from the client).
//!
//! Streams `BrowserServerMsg` frames from a per-activity `FrameRing` and
//! accepts `BrowserClientMsg::Subscribe` / `Resize` from the client. Task A9
//! wires `last_key` / `has_base_keyframe` so reconnects use `ResumeReplay`
//! instead of always re-sending the keyframe.

use crate::error::{HttpError, HttpResult};
use crate::state::AppState;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{FromRequest, Path, State, WebSocketUpgrade};
use axum::response::Response;
use ozmux_browser::cef_registry::BrowserCefRegistry;
use ozmux_browser::frame_ring::{FrameEnvelope, FrameSubscription, SubscribeRequest};
use ozmux_browser_cef_protocol::types::{ActivityId as CefActivityId, FrameKey};
use ozmux_browser_cef_protocol::wire::{
    BrowserClientMsg, BrowserServerMsg, FrameSubscriptionReply,
};
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};
use std::sync::Arc;

/// `GET /windows/{wid}/panes/{pid}/activities/{aid}/browser_cef/ws`
///
/// Validates origin, then upgrades to a WebSocket bound to the cef
/// `FrameRing` registered for `aid` under `state.browser_cef`. Closes
/// immediately if no ring is registered for that activity.
pub async fn browser_cef_ws(
    State(state): State<AppState>,
    Path((_wid, _pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
    req: axum::extract::Request,
) -> HttpResult<Response> {
    let origin = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !crate::origin_guard::is_allowed_origin(origin) {
        return Err(HttpError::ForbiddenOrigin);
    }
    let ws = WebSocketUpgrade::from_request(req, &())
        .await
        .map_err(|e| HttpError::Forbidden(e.to_string()))?;
    let registry = Arc::clone(&state.browser_cef);
    Ok(ws.on_upgrade(move |socket| run(socket, registry, aid)))
}

/// Captured subscribe-request shape coming off the wire.
struct SubscribeReq {
    session_id: Option<u64>,
    last_key: Option<FrameKey>,
    has_base_keyframe: bool,
}

async fn run(mut socket: WebSocket, registry: Arc<BrowserCefRegistry>, aid: ActivityId) {
    let aid_proto = CefActivityId(aid.to_string());
    let Some(ring) = registry.frame_ring(&aid_proto) else {
        tracing::debug!(?aid, "no cef FrameRing registered; closing");
        return;
    };
    let session_id_advertised = ring.session_id();

    let Some(req) = wait_for_subscribe(&mut socket).await else {
        return;
    };

    let sub = ring.subscribe(SubscribeRequest {
        session_id: req.session_id.unwrap_or(0),
        last_key: req.last_key,
        has_base_keyframe: req.has_base_keyframe,
    });

    // NOTE: MustRestart short-circuits — no backfill, no live stream. The
    // frontend handles the reply by dropping its renderer state and
    // re-subscribing with last_key=None.
    let (reply, replay_keyframe, replay_deltas, mut rx) = match sub {
        FrameSubscription::FreshSnapshot {
            keyframe,
            deltas,
            receiver,
        } => (
            FrameSubscriptionReply::FreshSnapshot,
            Some(keyframe),
            deltas,
            receiver,
        ),
        FrameSubscription::ResumeReplay { deltas, receiver } => {
            (FrameSubscriptionReply::ResumeReplay, None, deltas, receiver)
        }
        FrameSubscription::AwaitingKeyframe { receiver } => (
            FrameSubscriptionReply::AwaitingKeyframe,
            None,
            Vec::new(),
            receiver,
        ),
        FrameSubscription::MustRestart { reason } => {
            let msg = BrowserServerMsg::SubscribeReply {
                session_id: session_id_advertised,
                result: FrameSubscriptionReply::MustRestart { reason },
            };
            let _ = send_msg(&mut socket, &msg).await;
            return;
        }
    };

    if !send_msg(
        &mut socket,
        &BrowserServerMsg::SubscribeReply {
            session_id: session_id_advertised,
            result: reply,
        },
    )
    .await
    {
        return;
    }

    if let Some(kf) = replay_keyframe
        && !send_envelope(&mut socket, &kf).await
    {
        return;
    }
    for delta in &replay_deltas {
        if !send_envelope(&mut socket, delta).await {
            return;
        }
    }

    loop {
        match rx.recv().await {
            Ok(env) => {
                if !send_envelope(&mut socket, &env).await {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(?aid, lagged = n, "cef WS subscriber lagged");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn wait_for_subscribe(socket: &mut WebSocket) -> Option<SubscribeReq> {
    loop {
        match socket.recv().await {
            Some(Ok(Message::Binary(data))) => {
                match rmp_serde::from_slice::<BrowserClientMsg>(&data) {
                    Ok(BrowserClientMsg::Subscribe {
                        session_id,
                        last_key,
                        has_base_keyframe,
                    }) => {
                        return Some(SubscribeReq {
                            session_id,
                            last_key,
                            has_base_keyframe,
                        });
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        tracing::debug!(error = %e, "ignoring undecodable BrowserClientMsg");
                        continue;
                    }
                }
            }
            Some(Ok(Message::Close(_))) | None => return None,
            Some(Ok(_)) => continue,
            Some(Err(e)) => {
                tracing::debug!(error = %e, "cef WS recv error during subscribe wait");
                return None;
            }
        }
    }
}

async fn send_msg(socket: &mut WebSocket, msg: &BrowserServerMsg) -> bool {
    match rmp_serde::to_vec_named(msg) {
        Ok(bytes) => socket.send(Message::Binary(bytes.into())).await.is_ok(),
        Err(e) => {
            tracing::warn!(error = %e, "cef WS msgpack encode failed");
            true
        }
    }
}

async fn send_envelope(socket: &mut WebSocket, env: &Arc<FrameEnvelope>) -> bool {
    let msg = BrowserServerMsg::Screencast {
        session_id: env.session_id,
        epoch: env.epoch,
        frame_seq: env.frame_seq,
        captured_at_us: env.captured_at_us,
        width: env.width,
        height: env.height,
        is_keyframe: env.is_keyframe,
        damage_rects: env.damage_rects.clone(),
        bgra: env.bgra.clone(),
    };
    send_msg(socket, &msg).await
}
