use ozmux_extension::{handle::ExtensionHandles, registry::ExtensionRegistry, runtime::RuntimeRoot};
use ozmux_multiplexer::{activity::ActivityId, pane::PaneId};
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
    let _handles = ExtensionHandles::load(&runtime, ExtensionRegistry::default()).expect("spawn extension");

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
    let _handles = ExtensionHandles::load(&runtime, ExtensionRegistry::default()).expect("load extensions");
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
