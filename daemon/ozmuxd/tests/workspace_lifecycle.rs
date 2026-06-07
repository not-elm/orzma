//! Integration tests for CreateWorkspace / SelectWorkspace wire commands (P4c-1a T5).

use ozmux_mux::{MuxEvent, WorkspaceId};
use ozmux_proto::{Client, ClientMessage, ServerMessage};
use ozmuxd::Server;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

type ItClient = Client<UnixStream>;

fn sock(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ozmuxd-wl-{name}.sock"));
    let _ = std::fs::remove_file(&p);
    p
}

fn connect(path: &std::path::Path, viewport: (u16, u16)) -> ItClient {
    let stream = {
        let mut last_err = None;
        let mut s = None;
        for _ in 0..50 {
            match UnixStream::connect(path) {
                Ok(st) => {
                    s = Some(st);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        s.unwrap_or_else(|| panic!("connect failed: {last_err:?}"))
    };
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let reader = BufReader::new(stream.try_clone().unwrap());
    Client::connect(reader, stream, viewport).unwrap()
}

fn drain(c: &mut ItClient) {
    loop {
        match c.poll() {
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(e) => panic!("unexpected poll error during drain: {e}"),
        }
    }
}

/// Polls all `ServerMessage::Events` batches until `pred` returns `Some(T)` for
/// an event, or the deadline expires. Returns the extracted value on success.
fn poll_extract<T, F>(client: &mut ItClient, dur: Duration, mut pred: F) -> Option<T>
where
    F: FnMut(&MuxEvent) -> Option<T>,
{
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        match client.poll() {
            Ok(Some(ServerMessage::Events(batch))) => {
                for e in &batch {
                    if let Some(val) = pred(e) {
                        return Some(val);
                    }
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("unexpected poll error: {e}"),
        }
    }
    None
}

#[test]
fn create_workspace_named_emits_created_and_renamed() {
    let path = sock("create-named");
    let _server = Server::new().serve(&path).unwrap();
    let mut client = connect(&path, (80, 24));
    drain(&mut client);

    // Record the initial snapshot's first workspace id so we can confirm a new one appears.
    let before_snap = client.mirror().to_snapshot();
    let before_ws_count = before_snap.workspaces.len();

    client
        .send(ClientMessage::CreateWorkspace {
            name: Some("proj".into()),
        })
        .unwrap();

    // Collect events until we've seen both WorkspaceCreated and WorkspaceRenamed{name=proj}.
    let mut saw_created = false;
    let mut saw_renamed = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !(saw_created && saw_renamed) {
        match client.poll() {
            Ok(Some(ServerMessage::Events(batch))) => {
                for e in &batch {
                    if matches!(e, MuxEvent::WorkspaceCreated { .. }) {
                        saw_created = true;
                    }
                    if matches!(e, MuxEvent::WorkspaceRenamed { name, .. } if name == "proj") {
                        saw_renamed = true;
                    }
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("unexpected poll error: {e}"),
        }
    }

    assert!(
        saw_created,
        "expected WorkspaceCreated in Events batch after CreateWorkspace{{name:Some}}"
    );
    assert!(
        saw_renamed,
        "expected WorkspaceRenamed{{name=proj}} in Events batch after CreateWorkspace{{name:Some}}"
    );

    // The mirror now has one more workspace than before, AND the create+rename
    // batch folds to the renamed workspace at the Client level (not just the
    // events arriving): a single Events batch [WorkspaceCreated, .., WorkspaceRenamed]
    // must fold so the new workspace carries the name "proj".
    drain(&mut client);
    let after_snap = client.mirror().to_snapshot();
    assert_eq!(
        after_snap.workspaces.len(),
        before_ws_count + 1,
        "mirror should gain exactly one workspace"
    );
    assert!(
        after_snap.workspaces.iter().any(|ws| ws.name == "proj"),
        "the create+rename batch must fold to a workspace named \"proj\" at the Client level"
    );
}

#[test]
fn select_workspace_emits_workspace_selected() {
    let path = sock("select-ws");
    let _server = Server::new().serve(&path).unwrap();
    let mut client = connect(&path, (80, 24));
    drain(&mut client);

    // The first workspace in the mirror at this point is the original one.
    let original_ws: WorkspaceId = client.mirror().to_snapshot().workspaces[0].workspace;

    // Create a second workspace (unnamed) — it becomes the active one.
    client
        .send(ClientMessage::CreateWorkspace { name: None })
        .unwrap();

    // Wait until WorkspaceCreated arrives so we know the create completed.
    let new_ws = poll_extract(&mut client, Duration::from_secs(3), |e| match e {
        MuxEvent::WorkspaceCreated { workspace, .. } => Some(*workspace),
        _ => None,
    })
    .expect("no WorkspaceCreated after CreateWorkspace");

    // Drain remaining events from the create batch.
    drain(&mut client);

    // new_ws is now active; selecting original_ws is a genuine non-no-op switch.
    assert_ne!(
        new_ws, original_ws,
        "new workspace must differ from the original"
    );

    client
        .send(ClientMessage::SelectWorkspace {
            workspace: original_ws,
        })
        .unwrap();

    let got = poll_extract(&mut client, Duration::from_secs(3), |e| match e {
        MuxEvent::WorkspaceSelected { workspace, .. } if *workspace == original_ws => {
            Some(*workspace)
        }
        _ => None,
    });

    assert!(
        got.is_some(),
        "no WorkspaceSelected{{workspace=original_ws}} after SelectWorkspace"
    );
}
