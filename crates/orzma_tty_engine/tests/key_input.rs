//! Integration: trigger `TerminalKeyInput` on a live `TerminalBundle`
//! entity and verify the PTY child receives the bytes. Uses `/bin/cat`
//! as a minimal echo program — typing characters and pressing Enter
//! makes `cat` flush the line back through the PTY, which we observe
//! via the renderer-side `FrameSnapshot` / `FrameDelta` stream.

use bevy::ecs::observer::On;
use bevy::prelude::*;
use orzma_tty_engine::{
    SpawnOptions, TerminalBundle, TerminalHandlePlugin, TerminalKey, TerminalKeyInput,
    TerminalModifiers,
};
use orzma_tty_renderer::prelude::{FrameDelta, FrameSnapshot};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Resource, Clone, Default)]
struct CollectedText(Arc<Mutex<String>>);

fn collect_snapshot(snap: On<FrameSnapshot>, collected: Res<CollectedText>) {
    let mut out = collected.0.lock().unwrap();
    for row in snap.rows_data.iter() {
        for run in row.runs.iter() {
            out.push_str(&run.text);
        }
        out.push('\n');
    }
}

fn collect_delta(delta: On<FrameDelta>, collected: Res<CollectedText>) {
    let mut out = collected.0.lock().unwrap();
    for dirty in delta.dirty_rows.iter() {
        for run in dirty.runs.iter() {
            out.push_str(&run.text);
        }
        out.push('\n');
    }
}

fn pump_until<F: FnMut(&App) -> bool>(app: &mut App, deadline: Duration, mut done: F) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        app.update();
        if done(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn terminal_key_input_writes_encoded_bytes_to_pty() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(TerminalHandlePlugin);
    app.insert_resource(CollectedText::default());
    app.add_observer(collect_snapshot);
    app.add_observer(collect_delta);

    let bundle = TerminalBundle::spawn(SpawnOptions {
        cols: 40,
        rows: 5,
        shell: "/bin/cat".to_string(),
        cwd: None,
        env: Vec::new(),
    })
    .expect("spawn /bin/cat");
    let entity = app.world_mut().spawn(bundle).id();

    let got_snapshot = pump_until(&mut app, Duration::from_secs(2), |a| {
        !a.world()
            .resource::<CollectedText>()
            .0
            .lock()
            .unwrap()
            .is_empty()
    });
    assert!(got_snapshot, "initial snapshot did not arrive in 2s");

    app.world()
        .resource::<CollectedText>()
        .0
        .lock()
        .unwrap()
        .clear();

    app.world_mut().trigger(TerminalKeyInput {
        entity,
        key: TerminalKey::Text("h".into()),
        modifiers: TerminalModifiers::default(),
    });
    app.world_mut().trigger(TerminalKeyInput {
        entity,
        key: TerminalKey::Text("i".into()),
        modifiers: TerminalModifiers::default(),
    });
    app.world_mut().trigger(TerminalKeyInput {
        entity,
        key: TerminalKey::Enter,
        modifiers: TerminalModifiers::default(),
    });

    let got_echo = pump_until(&mut app, Duration::from_secs(5), |a| {
        let s = a.world().resource::<CollectedText>().0.lock().unwrap();
        s.contains("hi")
    });
    assert!(
        got_echo,
        "did not observe 'hi' echo within 5s; collected: {:?}",
        app.world().resource::<CollectedText>().0.lock().unwrap()
    );
}
