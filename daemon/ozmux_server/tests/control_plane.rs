//! Integration tests for the ozmux control-plane server over a real local socket.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use interprocess::local_socket::tokio::Stream;
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{GenericNamespaced, ToNsName};
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
    let frame = reader.next().await.expect("stream closed").expect("decode error");
    serde_json::from_slice(&frame).unwrap()
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
