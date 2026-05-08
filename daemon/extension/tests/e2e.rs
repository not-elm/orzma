use ozmux_extension::{handle::ExtensionHandles, runtime::RuntimeRoot};
use ozmux_session::{SessionId, activity::ActivityId, pane::PaneId, window::WindowId};
use ozmux_terminal::{SpawnOptions, TerminalService};
use std::{path::PathBuf, sync::Arc, time::Duration};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[tokio::test]
async fn pane_command_invokes_extension_handler() {
    let parent = tempfile::tempdir().unwrap();
    let runtime = Arc::new(RuntimeRoot::new_in(parent.path(), std::process::id()).unwrap());

    // Tell ExtensionHandles where to look:
    unsafe { std::env::set_var("OZMUX_EXTENSION_ROOT", fixture_root()); }
    let _handles = ExtensionHandles::load(&runtime).expect("spawn extension");

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
        activity.clone(),
        PaneId::new(),
        WindowId::new(),
        SessionId::new(),
        SpawnOptions { cols: 80, rows: 24, shell: "/bin/sh".to_string(), cwd: None },
    ).await.unwrap();

    let (_snap, mut rx) = svc.snapshot_and_subscribe(&activity).await.unwrap();
    svc.write(&activity, b"echoext alpha beta\n").await.unwrap();

    let mut got = Vec::new();
    let needle = b"ARGV=alpha,beta";
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(ozmux_terminal::TerminalEvent::Data { buffer })) => {
                got.extend_from_slice(&buffer);
                if got.windows(needle.len()).any(|w| w == needle) {
                    break;
                }
            }
            _ => {}
        }
    }
    let s = String::from_utf8_lossy(&got);
    assert!(s.contains("ARGV=alpha,beta"), "expected ARGV line, got: {s}");
    svc.kill(&activity).await.unwrap();
}
