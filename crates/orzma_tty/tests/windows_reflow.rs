//! End-to-end cover for reflowing a PowerShell session across a
//! narrow-then-wide resize under the ConPTY this machine provides.

#![cfg(windows)]

use orzma_tty::prelude::{OrzmaTty, TerminalKey, TerminalModifiers};
use orzma_tty::{CellPixels, NATIVE_SCROLLBACK_ON_GROW, SpawnOptions};
use orzma_vt::prelude::{Frame, GridSize, OrzmaVt, Row, Run, Scroll};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};
use std::{env, iter};

/// Viewport rows of the grid the test drives.
const ROWS: u16 = 24;

/// Columns of the grid before the narrowing and after the widening.
const WIDE: u16 = 80;

/// Columns of the narrowed grid, fewer than the prompt and every output
/// line take.
const NARROW: u16 = 40;

/// How many padded lines the command prints.
const LINES: usize = 30;

/// How many `x`s pad each output line past [`NARROW`] columns.
const PADDING: usize = 40;

/// The prompt the command installs, wider than [`NARROW`] columns and
/// narrower than [`WIDE`].
const PROMPT: &str = r"PS C:\orzma\reflow\a-prompt-wider-than-forty-columns> ";

/// The one-row line the command prints after the padded lines; with it,
/// the narrowing splits one padded line across the history/screen
/// boundary.
const TRAILER: &str = "done";

/// Asserts that across a narrow-then-wide resize the cursor stays on the
/// row orzma's own reflow chose, a wrapped output line and the prompt
/// rewrap and come back whole, and every output line survives exactly
/// once, including the one the narrowing split across the history/screen
/// boundary.
///
/// Case: a Windows user whose prompt and output lines are wider than half
/// the window narrows it to half its width and widens it back.
#[test]
fn a_narrow_then_wide_resize_keeps_the_cursor_row_and_every_line_once() {
    let shell = resolve_powershell();
    let size = GridSize::new(WIDE, ROWS).expect("a valid grid size");
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
    let command = format!(
        "function global:prompt {{ '{PROMPT}' }}; \
         1..{LINES} | ForEach-Object {{ \"line $_ \" + ('x' * {PADDING}) }}; '{TRAILER}'"
    );
    tty.send_paste(&command).expect("the command text");
    tty.send_key(&TerminalKey::Enter, &TerminalModifiers::default())
        .expect("the newline");
    wait_until(&mut tty, &mut mirror, |mirror| {
        mirror.rows.iter().any(|row| row == PROMPT.trim_end())
    });
    let last_line = format!("line {LINES} {}", "x".repeat(PADDING));
    for cols in [NARROW, WIDE] {
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
        for line in [last_line.as_str(), PROMPT.trim_end()] {
            let head: String = line.chars().take(usize::from(cols)).collect();
            assert!(
                mirror.rows.contains(&head),
                "no row holds the first {cols} columns of `{line}`:\n{:#?}",
                mirror.rows
            );
        }
        if cols == NARROW {
            let top = mirror.rows.first().expect("a screen row");
            assert!(
                !top.is_empty() && top.chars().all(|c| c == 'x'),
                "the top row at {cols} columns does not continue a line split into \
                 history:\n{:#?}",
                mirror.rows
            );
        }
    }
    let rows = every_row(&mut tty);
    let counts: Vec<usize> = (1..=LINES)
        .map(|n| {
            let prefix = format!("line {n} ");
            rows.iter().filter(|row| row.starts_with(&prefix)).count()
        })
        .collect();
    assert!(
        counts.iter().all(|count| *count == 1),
        "every output line appears exactly once: {counts:?}\n{rows:#?}"
    );
    let prompts = rows.iter().filter(|row| *row == PROMPT.trim_end()).count();
    assert_eq!(
        prompts, 1,
        "the prompt comes back whole on exactly one row:\n{rows:#?}"
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

/// Every history row followed by every screen row, read by paging the
/// viewport from the oldest history row down to the live tail.
fn every_row(tty: &mut OrzmaTty<OrzmaVt>) -> Vec<String> {
    let mut by_line: BTreeMap<i64, String> = BTreeMap::new();
    tty.scroll(Scroll::Top);
    loop {
        let output = tty.flush_now();
        let page = output.frames().last().expect("a scroll emits a frame");
        let offset = i64::from(page.display_offset.0);
        for row in &page.rows {
            by_line.insert(i64::from(row.line.0) - offset, text(&row.contents));
        }
        if offset == 0 {
            return by_line.into_values().collect();
        }
        tty.scroll(Scroll::PageDown);
    }
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
