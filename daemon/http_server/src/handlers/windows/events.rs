use crate::AppState;
use axum::extract::{
    Path, State, WebSocketUpgrade,
    ws::{CloseFrame, Message, WebSocket},
};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use ozmux_multiplexer::WindowId;

pub async fn events(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
    ws: WebSocketUpgrade,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_events_socket(socket, state, window_id))
}

async fn handle_events_socket(socket: WebSocket, state: AppState, window_id: WindowId) {
    let (mut tx, _rx) = socket.split();
    let snapshot_and_rx = state
        .with_window(&window_id, |w| super::window_view_for(w))
        .await;
    let Some(snapshot_result) = snapshot_and_rx else {
        close_with(&mut tx, 1011, "window_not_found").await;
        return;
    };
    let snapshot = match snapshot_result {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, %window_id, "snapshot build failed");
            close_with(&mut tx, 1011, "internal_error").await;
            return;
        }
    };
    let mut receiver = state.layout_broadcast.subscribe_or_create(&window_id);

    if tx
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    use tokio::sync::broadcast::error::RecvError;
    loop {
        match receiver.recv().await {
            Ok(view) => {
                if tx
                    .send(Message::Text(view.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(RecvError::Lagged(_)) => {
                close_with(&mut tx, 1011, "lagged").await;
                return;
            }
            Err(RecvError::Closed) => {
                close_with(&mut tx, 1011, "window_closed").await;
                return;
            }
        }
    }
}

async fn close_with(tx: &mut SplitSink<WebSocket, Message>, code: u16, reason: &'static str) {
    let _ = tx
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{bootstrap_default, fresh_state};
    use ozmux_multiplexer::{Activity, ActivityId, PaneId, Side, SplitOrientation};
    use std::net::SocketAddr;
    use tokio::net::TcpListener as TokioTcpListener;

    async fn spawn_server(state: crate::AppState) -> SocketAddr {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, crate::daemon_router(state))
                .await
                .unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn events_ws_sends_initial_snapshot() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};

        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let msg = ws.next().await.unwrap().unwrap();
        let text = match msg {
            TtMessage::Text(t) => t,
            other => panic!("expected text frame, got {other:?}"),
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["id"].as_str(), Some(wid.as_ref()));
        assert!(v["layout"].is_object());
    }

    #[tokio::test]
    async fn events_ws_closes_with_window_not_found_for_unknown_wid() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};
        let state = fresh_state();
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/windows/does-not-exist/events", addr);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        match ws.next().await.unwrap().unwrap() {
            TtMessage::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 1011);
                assert!(frame.reason.contains("window_not_found"));
            }
            other => panic!("expected close frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn events_ws_sends_frame_after_external_publish() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};

        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let bc = state.layout_broadcast.clone();
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let _initial = ws.next().await.unwrap().unwrap();

        bc.publish(
            &wid,
            serde_json::json!({ "id": wid.as_ref(), "marker": "second" }),
        );

        match ws.next().await.unwrap().unwrap() {
            TtMessage::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                assert_eq!(v["marker"].as_str(), Some("second"));
            }
            other => panic!("expected text frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn events_ws_closes_with_window_closed_when_broadcaster_drops() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let bc = state.layout_broadcast.clone();
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let _initial = ws.next().await.unwrap().unwrap();
        bc.close(&wid);
        match ws.next().await.unwrap().unwrap() {
            TtMessage::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 1011);
                assert!(frame.reason.contains("window_closed"));
            }
            other => panic!("expected close frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn events_ws_closes_with_lagged_when_subscriber_falls_behind() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};
        let state = crate::AppState {
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::new(1),
            ..fresh_state()
        };
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let bc = state.layout_broadcast.clone();
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let _initial = ws.next().await.unwrap().unwrap();
        bc.publish(&wid, serde_json::json!({ "n": 1 }));
        bc.publish(&wid, serde_json::json!({ "n": 2 }));
        bc.publish(&wid, serde_json::json!({ "n": 3 }));
        loop {
            match ws.next().await.unwrap().unwrap() {
                TtMessage::Close(Some(frame)) => {
                    assert_eq!(u16::from(frame.code), 1011);
                    assert!(frame.reason.contains("lagged"));
                    break;
                }
                TtMessage::Text(_) => continue,
                other => panic!("unexpected frame: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn snapshot_and_subscribe_is_atomic_under_concurrent_mutations() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};

        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let bc = state.layout_broadcast.clone();
        let state_for_server = state.clone();
        let addr = spawn_server(state_for_server).await;

        let mut handles = vec![];
        for _ in 0..5 {
            let s = state.clone();
            let bc = bc.clone();
            let wid_ = wid.clone();
            let pid_ = pid.clone();
            handles.push(tokio::spawn(async move {
                let new_pane_id = PaneId::new();
                let new_activity_id = ActivityId::new();
                let outcome = s
                    .with_window_or_404(&wid_, |w| {
                        w.split_pane(
                            &pid_,
                            new_pane_id.clone(),
                            Activity::terminal(new_activity_id.clone()),
                            Side::After,
                            SplitOrientation::Horizontal,
                        )
                    })
                    .await;
                if outcome.is_ok() {
                    s.pane_owner_window
                        .insert(new_pane_id.clone(), wid_.clone());
                    if let Some(view) = s
                        .with_window(&wid_, |w| crate::handlers::windows::window_view_for(w))
                        .await
                        && let Ok(view) = view
                    {
                        bc.publish(&wid_, view);
                    }
                }
            }));
        }

        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let _initial = ws.next().await.unwrap().unwrap();

        for h in handles {
            let _ = h.await;
        }

        let mut latest = match _initial {
            TtMessage::Text(t) => serde_json::from_str::<serde_json::Value>(&t).unwrap(),
            _ => panic!("expected text frame"),
        };
        while let Ok(Some(Ok(TtMessage::Text(t)))) =
            tokio::time::timeout(std::time::Duration::from_millis(100), ws.next()).await
        {
            latest = serde_json::from_str::<serde_json::Value>(&t).unwrap();
        }

        let final_pane_count = state
            .with_window(&wid, |w| w.panes.len())
            .await
            .unwrap_or(0);
        let observed_pane_count = latest["panes"].as_array().map(|a| a.len()).unwrap_or(0);
        assert_eq!(
            observed_pane_count, final_pane_count,
            "client's last observed view does not match multiplexer's final state \
             (observed={observed_pane_count}, final={final_pane_count})"
        );
    }
}
