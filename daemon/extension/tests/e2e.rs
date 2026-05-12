use ozmux_extension::{
    handle::ExtensionHandles, registry::ExtensionRegistry, runtime::RuntimeRoot,
};
use ozmux_multiplexer::{ActivityId, PaneId, SessionId, WindowId};
use ozmux_terminal::{SpawnOptions, TerminalService};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[tokio::test]
async fn pane_command_invokes_extension_handler() {
    let parent = tempfile::tempdir().unwrap();
    let runtime = Arc::new(RuntimeRoot::new_in(parent.path(), std::process::id()).unwrap());

    // Tell ExtensionHandles where to look:
    unsafe {
        std::env::set_var("OZMUX_EXTENSION_ROOT", fixture_root());
    }
    let _handles =
        ExtensionHandles::load(&runtime, ExtensionRegistry::default()).expect("spawn extension");

    // Wait for the extension to materialize its shim.
    let shim = runtime.bin_dir().join("echoext").join("echoext");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !shim.exists() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(shim.exists(), "shim never appeared at {:?}", shim);

    let svc = TerminalService::with_runtime_root(Arc::clone(&runtime));
    let activity = ActivityId::new();
    svc.spawn(
        PaneId::new(),
        activity.clone(),
        SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/sh".to_string(),
            cwd: None,
            window_id: Some(WindowId::new()),
            session_id: Some(SessionId::new()),
        },
    )
    .await
    .unwrap();

    let (_snap, mut rx) = svc.snapshot_and_subscribe(&activity).await.unwrap();
    svc.write(&activity, b"echoext alpha beta\n").await.unwrap();

    let mut got = Vec::new();
    let needle = b"ARGV=alpha,beta";
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Ok(ozmux_terminal::TerminalEvent::Data { buffer })) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            got.extend_from_slice(&buffer);
            if got.windows(needle.len()).any(|w| w == needle) {
                break;
            }
        }
    }
    let s = String::from_utf8_lossy(&got);
    assert!(
        s.contains("ARGV=alpha,beta"),
        "expected ARGV line, got: {s}"
    );
    svc.kill(&activity).await.unwrap();
}

#[tokio::test]
async fn load_pre_creates_extension_bin_dirs() {
    let parent = tempfile::tempdir().unwrap();
    let runtime = Arc::new(RuntimeRoot::new_in(parent.path(), std::process::id()).unwrap());
    unsafe {
        std::env::set_var("OZMUX_EXTENSION_ROOT", fixture_root());
    }
    let _handles =
        ExtensionHandles::load(&runtime, ExtensionRegistry::default()).expect("load extensions");
    for name in ["echoext", "crashext"] {
        let bin_dir = runtime.bin_dir().join(name);
        assert!(
            bin_dir.is_dir(),
            "expected bin dir {bin_dir:?} to exist immediately after load"
        );
    }
}

#[tokio::test]
async fn daemon_drop_removes_runtime_root() {
    let parent = tempfile::tempdir().unwrap();
    let path;
    {
        let runtime = RuntimeRoot::new_in(parent.path(), std::process::id() + 1).unwrap();
        path = runtime.root().to_path_buf();
        assert!(path.exists());
    }
    assert!(
        !path.exists(),
        "expected runtime root to be cleaned up by Drop"
    );
}

#[tokio::test]
async fn extension_crash_does_not_break_other_extensions() {
    let parent = tempfile::tempdir().unwrap();
    let runtime = Arc::new(RuntimeRoot::new_in(parent.path(), std::process::id()).unwrap());
    unsafe {
        std::env::set_var("OZMUX_EXTENSION_ROOT", fixture_root());
    }
    let _handles = ExtensionHandles::load(&runtime, ExtensionRegistry::default()).unwrap();

    let echo = runtime.bin_dir().join("echoext").join("echoext");
    let crash = runtime.bin_dir().join("crashext").join("crashext");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !(echo.exists() && crash.exists()) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(echo.exists() && crash.exists());

    // Wait for the crash extension to die (its setTimeout exits after 200ms).
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Echo extension's bin dir must still exist.
    assert!(
        echo.exists(),
        "echo bin disappeared after crash extension died"
    );
}

#[tokio::test]
async fn extension_streams_channel_events() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let parent = tempfile::tempdir().unwrap();
    let runtime = Arc::new(RuntimeRoot::new_in(parent.path(), std::process::id()).unwrap());
    unsafe {
        std::env::set_var("OZMUX_EXTENSION_ROOT", fixture_root());
    }
    let registry = ExtensionRegistry::default();
    let _handles = ExtensionHandles::load(&runtime, registry.clone()).expect("spawn extensions");

    // Wait for clock-ext's handlers UDS to appear in the registry.
    let deadline = Instant::now() + Duration::from_secs(5);
    let sock = loop {
        if let Some(p) = registry.handlers_sock_path("clock-ext")
            && p.exists()
        {
            break p;
        }
        if Instant::now() > deadline {
            panic!("clock-ext handlers sock never appeared");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // The fixture pre-registers channels against the hardcoded aid
    // "fixture-aid-1" before calling bootstrap(), so no daemon HTTP server
    // is needed for this test.
    let aid = "fixture-aid-1";

    let stream = UnixStream::connect(&sock).await.unwrap();
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    let open = serde_json::json!({
        "aid": aid,
        "frame": {
            "kind": "sub.open",
            "id": "s1",
            "name": "ticks",
            "params": { "n": 3 },
        },
    })
    .to_string()
        + "\n";
    write_half.write_all(open.as_bytes()).await.unwrap();

    let mut received = Vec::new();
    for _ in 0..4 {
        let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        received.push(v);
    }
    assert_eq!(received[0]["frame"]["kind"], "sub.data");
    assert_eq!(received[0]["frame"]["payload"], serde_json::json!({"i": 0}));
    assert_eq!(received[1]["frame"]["payload"], serde_json::json!({"i": 1}));
    assert_eq!(received[2]["frame"]["payload"], serde_json::json!({"i": 2}));
    assert_eq!(received[3]["frame"]["kind"], "sub.complete");
}
