//! Integration tests for the ozmux control-plane server over a real local socket.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use interprocess::local_socket::tokio::Stream;
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use ozmux_mux::{MuxEvent, PaneId, Side, SplitOrientation, SurfaceKind};
use ozmux_proto::{ClientMessage, MAX_MESSAGE_BYTES, ServerMessage};
use ozmux_server::OzmuxServer;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::task::JoinHandle;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

type ClientReader = FramedRead<interprocess::local_socket::tokio::RecvHalf, LengthDelimitedCodec>;
type ClientWriter = FramedWrite<interprocess::local_socket::tokio::SendHalf, LengthDelimitedCodec>;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_name() -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("ozmux-test-{}-{}.sock", std::process::id(), n)
}

fn codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .max_frame_length(MAX_MESSAGE_BYTES as usize)
        .new_codec()
}

fn spawn_server() -> (String, JoinHandle<anyhow::Result<()>>) {
    let name = unique_name();
    let server = OzmuxServer::new(&name).unwrap();
    let handle = tokio::spawn(async move { server.start().await });
    (name, handle)
}

async fn connect_client(name: &str) -> (ClientReader, ClientWriter) {
    let nsname = name.to_ns_name::<GenericNamespaced>().unwrap();
    let stream = Stream::connect(nsname).await.unwrap();
    let (read_half, write_half) = stream.split();
    (
        FramedRead::new(read_half, codec()),
        FramedWrite::new(write_half, codec()),
    )
}

async fn send(writer: &mut ClientWriter, msg: ClientMessage) {
    let body = serde_json::to_vec(&msg).unwrap();
    writer.send(Bytes::from(body)).await.unwrap();
}

async fn recv(reader: &mut ClientReader) -> ServerMessage {
    let frame = reader
        .next()
        .await
        .expect("stream closed")
        .expect("decode error");
    serde_json::from_slice(&frame).unwrap()
}

async fn recv_events(reader: &mut ClientReader) -> Vec<MuxEvent> {
    loop {
        match recv(reader).await {
            ServerMessage::Events(events) => return events,
            ServerMessage::Frame { .. } | ServerMessage::SurfaceEvent { .. } => continue,
            other => panic!("expected Events, got {other:?}"),
        }
    }
}

async fn recv_error(reader: &mut ClientReader) {
    loop {
        match recv(reader).await {
            ServerMessage::Error { .. } => return,
            ServerMessage::Frame { .. } | ServerMessage::SurfaceEvent { .. } => continue,
            other => panic!("expected Error, got {other:?}"),
        }
    }
}

fn active_pane_of(welcome: &ServerMessage) -> PaneId {
    match welcome {
        ServerMessage::Welcome { snapshot } => {
            let ws = snapshot
                .workspaces
                .iter()
                .find(|w| w.workspace == snapshot.active_workspace)
                .expect("active workspace present");
            ws.active_pane
        }
        other => panic!("expected Welcome, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_receives_welcome_matching_snapshot() {
    let (name, _server) = spawn_server();
    let (mut reader, _writer) = connect_client(&name).await;
    match recv(&mut reader).await {
        ServerMessage::Welcome { snapshot } => {
            assert_eq!(snapshot.workspaces.len(), 1);
            let ws = &snapshot.workspaces[0];
            assert_eq!(ws.workspace, snapshot.active_workspace);
            assert_eq!(ws.panes.len(), 1);
            assert_eq!(ws.panes[0].pane, ws.active_pane);
            assert_eq!(ws.panes[0].surfaces.len(), 1);
        }
        other => panic!("expected Welcome, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_stops_the_server() {
    let (name, server) = spawn_server();
    let (mut reader, mut writer) = connect_client(&name).await;
    let _welcome = recv(&mut reader).await;
    send(&mut writer, ClientMessage::Shutdown).await;
    let joined = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
    let result = joined
        .expect("server did not stop within 5s")
        .expect("server task panicked");
    assert!(result.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_broadcasts_events_to_sender() {
    let (name, _server) = spawn_server();
    let (mut reader, mut writer) = connect_client(&name).await;
    let welcome = recv(&mut reader).await;
    let pane = active_pane_of(&welcome);
    send(
        &mut writer,
        ClientMessage::Split {
            pane,
            orientation: SplitOrientation::Horizontal,
            side: Side::After,
            kind: SurfaceKind::Terminal,
            cwd: None,
        },
    )
    .await;
    let events = recv_events(&mut reader).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, MuxEvent::PaneCreated { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, MuxEvent::LayoutChanged { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, MuxEvent::ActivePaneChanged { .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_is_broadcast_to_all_clients() {
    let (name, _server) = spawn_server();
    let (mut reader_a, mut writer_a) = connect_client(&name).await;
    let (mut reader_b, _writer_b) = connect_client(&name).await;
    let welcome_a = recv(&mut reader_a).await;
    let _welcome_b = recv(&mut reader_b).await;
    let pane = active_pane_of(&welcome_a);
    send(
        &mut writer_a,
        ClientMessage::Split {
            pane,
            orientation: SplitOrientation::Vertical,
            side: Side::After,
            kind: SurfaceKind::Terminal,
            cwd: None,
        },
    )
    .await;
    for reader in [&mut reader_a, &mut reader_b] {
        let events = recv_events(reader).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, MuxEvent::PaneCreated { .. }))
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_last_pane_errors_only_to_sender() {
    let (name, _server) = spawn_server();
    let (mut reader_a, mut writer_a) = connect_client(&name).await;
    let (mut reader_b, _writer_b) = connect_client(&name).await;
    let welcome_a = recv(&mut reader_a).await;
    let _welcome_b = recv(&mut reader_b).await;
    let pane = active_pane_of(&welcome_a);
    send(&mut writer_a, ClientMessage::Close { pane }).await;
    recv_error(&mut reader_a).await;
    // NOTE: terminal drivers broadcast Frame/SurfaceEvent messages continuously;
    // drain those and assert no Error arrives within the window.
    let deadline = std::time::Duration::from_millis(300);
    loop {
        match tokio::time::timeout(deadline, recv(&mut reader_b)).await {
            Err(_) => break,
            Ok(ServerMessage::Frame { .. } | ServerMessage::SurfaceEvent { .. }) => continue,
            Ok(other) => panic!("an error must not broadcast to other clients; got {other:?}"),
        }
    }
}

#[ignore = "replaced by data-plane input test in Unit 7"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_is_rejected_with_error() {
    let (name, _server) = spawn_server();
    let (mut reader, mut writer) = connect_client(&name).await;
    let welcome = recv(&mut reader).await;
    let surface = match &welcome {
        ServerMessage::Welcome { snapshot } => snapshot.workspaces[0].panes[0].active_surface,
        other => panic!("expected Welcome, got {other:?}"),
    };
    send(
        &mut writer,
        ClientMessage::Input {
            surface,
            bytes: vec![b'x'],
        },
    )
    .await;
    match recv(&mut reader).await {
        ServerMessage::Error { message } => assert!(message.contains("control-plane")),
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_workspace_broadcasts_created_with_name() {
    let (name, _server) = spawn_server();
    let (mut reader, mut writer) = connect_client(&name).await;
    let _welcome = recv(&mut reader).await;
    send(
        &mut writer,
        ClientMessage::CreateWorkspace {
            name: Some("proj".to_string()),
        },
    )
    .await;
    let events = recv_events(&mut reader).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, MuxEvent::WorkspaceCreated { name, .. } if name == "proj")),
        "the requested name arrives atomically on WorkspaceCreated"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, MuxEvent::WorkspaceRenamed { .. })),
        "naming is atomic at creation; no separate rename event"
    );
}
