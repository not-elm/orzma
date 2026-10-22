//! End-to-end cover for the order in which the ConPTY delivers a synchronized update that scrolls the screen.

#![cfg(windows)]

#[path = "support/powershell.rs"]
mod powershell;

use orzma_tty::prelude::{OrzmaTty, TerminalKey, TerminalModifiers};
use orzma_tty::{CellPixels, EnvKey, EnvValue, NATIVE_SCROLLBACK_ON_GROW, SpawnOptions};
use orzma_vt::prelude::{Frame, GridSize, OrzmaVt, Row, Run};
use powershell::resolve_powershell;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tempfile::TempDir;

/// Viewport rows of the grid the payload scrolls.
const ROWS: u16 = 24;

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

/// Asserts that no frame shows a closed synchronized update one row short
/// of its final screen, under the ConPTY orzma bundles.
///
/// Case: a full-screen TUI in a split pane redraws its whole frame inside a
/// synchronized update that scrolls the pane, as Ink does on Windows on
/// every spinner tick.
#[test]
fn a_scrolling_synchronized_update_reaches_its_last_line_in_one_frame() {
    /// Columns of the grid the payload scrolls.
    const COLS: u16 = 80;

    let (shell, _) = resolve_powershell();
    let home = TempDir::new().expect("a temporary home");
    let size = GridSize::new(COLS, ROWS).expect("a valid grid size");
    let mut tty = OrzmaTty::spawn(
        OrzmaVt::new(size, 1000).with_scrollback_on_grow(NATIVE_SCROLLBACK_ON_GROW),
        SpawnOptions {
            size,
            cell_px: CellPixels::default(),
            shell: shell.display().to_string(),
            cwd: None,
            env: vec![(
                EnvKey("USERPROFILE".to_string()),
                EnvValue(home.path().display().to_string()),
            )],
            shell_integration: false,
        },
    )
    .expect("a spawned shell");
    let mut mirror = Mirror::new();
    let prompt_deadline = Instant::now() + Duration::from_secs(60);
    while !mirror.rows.iter().any(|row| row.ends_with('>')) {
        assert!(
            Instant::now() < prompt_deadline,
            "timed out:\n{:#?}",
            mirror.rows
        );
        let output = tty.pump();
        for frame in output.frames() {
            mirror.apply(frame);
        }
        sleep(Duration::from_millis(20));
    }
    let expected_host: PathBuf = std::env::current_exe()
        .expect("the test executable path")
        .parent()
        .expect("the deps directory")
        .parent()
        .expect("the debug directory")
        .join("OpenConsole.exe");
    let current = Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    let host: Option<PathBuf> = system
        .processes()
        .values()
        .filter(|process| process.parent() == Some(current))
        .find_map(|process| {
            let exe = process.exe()?;
            let file = exe.file_name()?;
            if file.eq_ignore_ascii_case("OpenConsole.exe")
                || file.eq_ignore_ascii_case("conhost.exe")
            {
                Some(exe.to_path_buf())
            } else {
                None
            }
        });
    let command = r#"$e=[char]27; [Console]::Write("ready`r`n"); Start-Sleep -Milliseconds 500; foreach ($r in 1..5) { [Console]::Write("$e[?2026h"); [Console]::Write("$e[2J$e[3J$e[H"); foreach ($i in 1..30) { [Console]::Write("round $r line $i`r`n") }; [Console]::Write("round $r last line"); [Console]::Write("$e[?2026l"); Start-Sleep -Milliseconds 200 }"#;
    tty.send_paste(command).expect("the command text");
    tty.send_key(&TerminalKey::Enter, &TerminalModifiers::default())
        .expect("the newline");
    let mut screens: Vec<Vec<String>> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut finished_at: Option<Instant> = None;
    loop {
        if Instant::now() >= deadline {
            break;
        }
        let output = tty.pump();
        for frame in output.frames() {
            mirror.apply(frame);
            screens.push(mirror.rows.clone());
            if mirror
                .rows
                .last()
                .is_some_and(|row| row == "round 5 last line")
                && finished_at.is_none()
            {
                finished_at = Some(Instant::now());
            }
        }
        if finished_at.is_some_and(|at| at.elapsed() > Duration::from_millis(500)) {
            break;
        }
        sleep(Duration::from_millis(20));
    }
    let _ = mirror.cursor_line;
    // `host: Option<PathBuf>` and `expected_host: PathBuf` recorded before the paste;
    // `screens: Vec<Vec<String>>` holds the mirror's rows after each frame.
    for (index, rows) in screens.iter().enumerate() {
        for (line, row) in rows.iter().enumerate() {
            let Some(round) = row
                .strip_prefix("round ")
                .and_then(|rest| rest.strip_suffix(" line 30"))
            else {
                continue;
            };
            assert_eq!(
                rows.get(line + 1).map(String::as_str),
                Some(format!("round {round} last line").as_str()),
                "frame {index} ends one row short of a closed synchronized update:\n{rows:#?}"
            );
        }
    }
    assert!(
        screens
            .iter()
            .any(|rows| rows.last().is_some_and(|row| row == "round 5 last line")),
        "the payload never finished: {:#?}",
        screens.last()
    );
    assert_eq!(
        host.as_deref(),
        Some(expected_host.as_path()),
        "panes run under {host:?}, not the bundled ConPTY; run `just build` \
         (or `python scripts/conpty.py install --dest target/debug`) first"
    );
}
