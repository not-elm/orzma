//! End-to-end cover for reflowing a PowerShell session across a
//! narrow-then-wide resize under the ConPTY this machine provides.

#![cfg(windows)]

use orzma_tty::prelude::{OrzmaTty, TerminalKey, TerminalModifiers};
use orzma_tty::{CellPixels, NATIVE_SCROLLBACK_ON_GROW, SpawnOptions};
use orzma_vt::prelude::{Frame, GridSize, OrzmaVt, Row, Run, Scroll};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};
use std::{env, iter};

/// Viewport rows of the grid the test drives.
const ROWS: u16 = 24;

/// Asserts that after a narrow-then-wide resize the cursor sits on the
/// row orzma's own reflow put it on, and every output line survives
/// exactly once.
///
/// Case: a Windows user with a screenful of output narrows the window to
/// half its width and widens it back.
#[test]
fn a_narrow_then_wide_resize_keeps_the_cursor_row_and_every_line_once() {
    let shell = resolve_powershell();
    let size = GridSize::new(80, ROWS).expect("a valid grid size");
    let mut tty = OrzmaTty::spawn(
        OrzmaVt::new(size, 1000).with_scrollback_on_grow(NATIVE_SCROLLBACK_ON_GROW),
        SpawnOptions {
            size,
            cell_px: CellPixels::default(),
            shell: shell.display().to_string(),
            cwd: None,
            env: vec![],
            shell_integration: false,
        },
    )
    .expect("a spawned shell");
    let mut mirror = Mirror::new();
    wait_until(&mut tty, &mut mirror, |mirror| {
        mirror.rows.iter().any(|row| row.ends_with('>'))
    });
    tty.send_paste("1..30 | ForEach-Object { \"line $_\" }")
        .expect("the command text");
    tty.send_key(&TerminalKey::Enter, &TerminalModifiers::default())
        .expect("the newline");
    wait_until(&mut tty, &mut mirror, |mirror| {
        mirror.rows.iter().any(|row| row == "line 30")
    });
    for cols in [40, 80] {
        let size = GridSize::new(cols, ROWS).expect("a valid grid size");
        tty.resize(size, CellPixels::default()).expect("the resize");
        let reflowed = tty.flush_now();
        let frame = reflowed.frames().last().expect("a resize emits a frame");
        mirror.apply(frame);
        let own_line = frame.cursor.point.line.0;
        settle(&mut tty, &mut mirror);
        assert_eq!(
            mirror.cursor_line, own_line,
            "ConPTY moved the cursor off the row orzma's reflow chose at {cols} columns"
        );
    }
    let rows = every_row(&mut tty);
    let counts: Vec<usize> = (1..=30)
        .map(|n| {
            rows.iter()
                .filter(|row| **row == format!("line {n}"))
                .count()
        })
        .collect();
    assert!(
        counts.iter().all(|count| *count == 1),
        "every output line appears exactly once: {counts:?}\n{rows:#?}"
    );
}

/// The viewport rows the frames so far have painted, and where the
/// cursor stands.
struct Mirror {
    rows: Vec<String>,
    cursor_line: i32,
}

impl Mirror {
    fn new() -> Self {
        Self {
            rows: vec![String::new(); usize::from(ROWS)],
            cursor_line: 0,
        }
    }

    fn apply(&mut self, frame: &Frame) {
        self.rows
            .resize(usize::from(frame.size.rows), String::new());
        for row in &frame.rows {
            if let Some(slot) = self.rows.get_mut(usize::from(row.line.0)) {
                *slot = text(&row.contents);
            }
        }
        self.cursor_line = frame.cursor.point.line.0;
    }
}

fn text(row: &Row<Run>) -> String {
    row.iter()
        .map(|run| run.text.as_str())
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Pumps until `done` holds for the mirror, then until the output goes
/// quiet.
fn wait_until(tty: &mut OrzmaTty<OrzmaVt>, mirror: &mut Mirror, done: impl Fn(&Mirror) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done(mirror) {
        assert!(Instant::now() < deadline, "timed out:\n{:#?}", mirror.rows);
        let output = tty.pump();
        for frame in output.frames() {
            mirror.apply(frame);
        }
        sleep(Duration::from_millis(20));
    }
    settle(tty, mirror);
}

/// Pumps until no frame has arrived for a second and a half.
fn settle(tty: &mut OrzmaTty<OrzmaVt>, mirror: &mut Mirror) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut quiet_since = Instant::now();
    while Instant::now() < deadline {
        let output = tty.pump();
        let mut painted = false;
        for frame in output.frames() {
            mirror.apply(frame);
            painted = true;
        }
        if painted {
            quiet_since = Instant::now();
        } else if quiet_since.elapsed() > Duration::from_millis(1500) {
            return;
        }
        sleep(Duration::from_millis(20));
    }
}

/// Every history row followed by every screen row, read by scrolling the
/// viewport to the top and back.
fn every_row(tty: &mut OrzmaTty<OrzmaVt>) -> Vec<String> {
    tty.scroll(Scroll::Top);
    let top_output = tty.flush_now();
    let top = top_output.frames().last().expect("a scroll emits a frame");
    let history = usize::try_from(top.display_offset.0).expect("history fits usize");
    assert!(
        history <= usize::from(ROWS),
        "the history outgrew one viewport"
    );
    let mut above = Mirror::new();
    above.apply(top);
    tty.scroll(Scroll::Bottom);
    let bottom_output = tty.flush_now();
    let bottom = bottom_output
        .frames()
        .last()
        .expect("a scroll emits a frame");
    let mut live = Mirror::new();
    live.apply(bottom);
    above.rows.truncate(history);
    above.rows.into_iter().chain(live.rows).collect()
}

/// The PowerShell executable this test runs against, preferring `pwsh`.
fn resolve_powershell() -> PathBuf {
    on_path("pwsh")
        .or_else(|| on_path("powershell"))
        .expect("neither pwsh nor powershell is on PATH")
}

/// The full path `name` resolves to through `PATH` and `PATHEXT`.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let extensions: Vec<OsString> = env::var("PATHEXT")
        .ok()
        .map(|v| {
            v.split(';')
                .filter(|e| !e.is_empty())
                .map(OsString::from)
                .collect()
        })
        .unwrap_or_else(|| vec![OsString::from(".EXE")]);
    env::split_paths(&path).find_map(|dir| candidate(&dir, name, &extensions))
}

/// The first spelling of `name` under `dir` that names a file.
fn candidate(dir: &Path, name: &str, extensions: &[OsString]) -> Option<PathBuf> {
    iter::once(dir.join(name))
        .chain(extensions.iter().map(|ext| {
            let mut file = OsString::from(name);
            file.push(ext);
            dir.join(file)
        }))
        .find(|path| path.is_file())
}
