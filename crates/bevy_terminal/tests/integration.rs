//! Integration: a minimal Bevy `App` with `TerminalHandlePlugin`
//! spawns a `TerminalBundle` running `/bin/sh` (interactive) and
//! verifies a `FrameSnapshot` trigger arrives within N ticks. The
//! test exercises the bootstrap-snapshot rescue path in
//! `check_deadline_flush` — alacritty's first `damage()` returns
//! `Full`, so the initial snapshot is emitted regardless of whether
//! the shell produces PTY output before the assertion window
//! elapses.

use bevy::ecs::observer::On;
use bevy::prelude::*;
use bevy_terminal::{SpawnOptions, TerminalBundle, TerminalHandlePlugin};
use bevy_terminal_renderer::prelude::FrameSnapshot;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Resource, Clone)]
struct SnapshotsSeen(Arc<Mutex<u32>>);

fn observe_snapshot(snap: On<FrameSnapshot>, counter: Res<SnapshotsSeen>) {
    let _ = snap.entity;
    *counter.0.lock().unwrap() += 1;
}

#[test]
fn bundle_emits_initial_snapshot_within_a_few_ticks() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(TerminalHandlePlugin);
    let counter = Arc::new(Mutex::new(0u32));
    app.insert_resource(SnapshotsSeen(counter.clone()));
    app.add_observer(observe_snapshot);

    let bundle = TerminalBundle::spawn(SpawnOptions {
        cols: 20,
        rows: 5,
        shell: "/bin/sh".to_string(),
        cwd: None,
        env: vec![("PS1".into(), "$ ".into())],
    })
    .expect("spawn shell");
    app.world_mut().spawn(bundle);

    for _ in 0..120 {
        app.update();
        if *counter.lock().unwrap() >= 1 {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    panic!(
        "no FrameSnapshot triggered within 120 ticks (count={})",
        *counter.lock().unwrap()
    );
}
