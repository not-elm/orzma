//! WebSocket handler for cef-backed BrowserActivities.
//!
//! Streams `BrowserServerMsg` frames from a per-activity `FrameRing` and
//! accepts `BrowserClientMsg::Subscribe` / `Resize` / `Input` / `Navigate` /
//! `NavigateHistory` from the client. A concurrent `select!` arm drains
//! inbound `BrowserClientMsg` frames and forwards them to
//! `CefDispatcher::dispatch`.

use crate::error::{HttpError, HttpResult};
use crate::state::AppState;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{FromRequest, Path, State, WebSocketUpgrade};
use axum::response::Response;
use ozmux_browser::cef_dispatcher::CefDispatcher;
use ozmux_browser::cef_registry::{BrowserCefRegistry, NavState};
use ozmux_browser::frame_ring::{FrameEnvelope, FrameSubscription, SubscribeRequest};
use ozmux_browser_cef_protocol::types::{ActivityId as CefActivityId, FrameKey};
use ozmux_browser_cef_protocol::wire::{
    BrowserClientMsg, BrowserServerMsg, FrameSubscriptionReply, HostCommand,
};
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};
use std::sync::Arc;
use tokio::sync::broadcast;

/// `GET /windows/{wid}/panes/{pid}/activities/{aid}/browser/ws`
///
/// Validates origin, then upgrades to a WebSocket bound to the cef
/// `FrameRing` registered for `aid` under `state.browser_cef`. Closes
/// immediately if no ring is registered for that activity.
pub async fn browser_ws(
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
    let cef_host = Arc::clone(&state.cef_host);
    Ok(ws.on_upgrade(move |socket| run(socket, registry, cef_host, aid)))
}

/// Captured subscribe-request shape coming off the wire.
struct SubscribeReq {
    session_id: Option<u64>,
    last_key: Option<FrameKey>,
    has_base_keyframe: bool,
}

async fn run(
    mut socket: WebSocket,
    registry: Arc<BrowserCefRegistry>,
    cef_host: Arc<dyn CefDispatcher>,
    aid: ActivityId,
) {
    let aid_proto = CefActivityId(aid.to_string());
    let Some(ring) = registry.frame_ring(&aid_proto) else {
        tracing::debug!(?aid, "no cef FrameRing registered; closing");
        return;
    };
    let Some(mut nav_rx) = registry.nav_subscribe(&aid_proto) else {
        tracing::debug!(?aid, "no cef NavState channel registered; closing");
        return;
    };
    let Some(mut cursor_rx) = registry.cursor_subscribe(&aid_proto) else {
        tracing::debug!(?aid, "no cef cursor channel registered; closing");
        return;
    };
    let mut unavailable_rx = registry.unavailable_subscribe();
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

    // Drive outbound (screencast frames + nav updates) and inbound (client
    // messages) concurrently from a single select! loop, keeping one socket.
    loop {
        tokio::select! {
            // Outbound: deliver the next screencast frame to the client.
            recv_result = rx.recv() => {
                match recv_result {
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
            // Inbound: read further BrowserClientMsg frames and forward to cef_host.
            ws_result = socket.recv() => {
                match ws_result {
                    Some(Ok(Message::Binary(data))) => {
                        forward_client_msg(&data, &aid_proto, &*cef_host).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::debug!(error = %e, "cef WS recv error in main loop");
                        break;
                    }
                }
            }
            // Outbound: push a Nav message whenever nav state changes.
            nav_result = nav_rx.changed() => {
                match nav_result {
                    Ok(()) => {
                        let state: NavState = nav_rx.borrow().clone();
                        let msg = BrowserServerMsg::Nav {
                            url: state.url,
                            title: state.title,
                            can_back: state.can_back,
                            can_forward: state.can_forward,
                        };
                        if !send_msg(&mut socket, &msg).await {
                            break;
                        }
                    }
                    Err(_) => {
                        // NOTE: Err means the watch::Sender was dropped (activity removed
                        // from registry). Treat as a clean close.
                        break;
                    }
                }
            }
            // Outbound: push a Cursor message whenever the page cursor changes.
            cursor_result = cursor_rx.changed() => {
                match cursor_result {
                    Ok(()) => {
                        let cursor = *cursor_rx.borrow();
                        if !send_msg(&mut socket, &BrowserServerMsg::Cursor { cursor }).await {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            // Outbound: forward BrowserUnavailable to the client and close.
            unavailable_result = unavailable_rx.recv() => {
                match unavailable_result {
                    Ok(reason) => {
                        send_msg(&mut socket, &BrowserServerMsg::BrowserUnavailable { reason }).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

/// Decode a raw binary frame received after the initial Subscribe and forward
/// the corresponding `HostCommand` to cef_host. Unrecognised or irrelevant
/// variants are silently ignored.
async fn forward_client_msg(data: &[u8], aid_proto: &CefActivityId, cef_host: &dyn CefDispatcher) {
    let cm = match rmp_serde::from_slice::<BrowserClientMsg>(data) {
        Ok(cm) => cm,
        Err(e) => {
            tracing::debug!(error = %e, "ignoring undecodable BrowserClientMsg");
            return;
        }
    };
    let cmd = match cm {
        BrowserClientMsg::Input { event } => HostCommand::SendInput {
            aid: aid_proto.clone(),
            input: event,
        },
        BrowserClientMsg::Navigate { url } => HostCommand::Navigate {
            aid: aid_proto.clone(),
            url,
        },
        BrowserClientMsg::NavigateHistory { delta } => HostCommand::NavigateHistory {
            aid: aid_proto.clone(),
            delta,
        },
        BrowserClientMsg::Resize { css_w, css_h, dpr } => HostCommand::Resize {
            aid: aid_proto.clone(),
            css_w,
            css_h,
            dpr,
        },
        // Subscribe is handled before the main loop; CopyRequest and Paste
        // require a separate channel not yet plumbed in Phase B.
        BrowserClientMsg::Subscribe { .. }
        | BrowserClientMsg::CopyRequest
        | BrowserClientMsg::Paste { .. } => return,
    };
    if let Err(e) = cef_host.dispatch(cmd) {
        tracing::debug!(error = %e, "cef_host dispatch failed");
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
        is_popup: env.is_popup,
        // NOTE: FrameEnvelope does not carry popup_rect — the daemon event
        // pump does not yet read popup frames from shm and route them.
        popup_rect: None,
        bgra: env.bgra.clone(),
    };
    send_msg(socket, &msg).await
}
