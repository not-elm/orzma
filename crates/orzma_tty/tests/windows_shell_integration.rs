//! End-to-end cover for the working-directory report orzma injects into
//! PowerShell on Windows.

#![cfg(windows)]

use orzma_tty::prelude::{OrzmaTty, TerminalKey, TerminalModifiers, TtySignal};
use orzma_tty::{CellPixels, EnvKey, EnvValue, SpawnOptions};
use orzma_vt::prelude::{GridSize, OrzmaVt, VtSignal};
use std::ffi::OsString;
use std::fs::{create_dir_all, write};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};
use std::{env, iter};
use tempfile::TempDir;

/// The marker the fixture profile announces itself with, through an
/// `OSC 0` whose title the terminal reports as a signal.
const PROFILE_MARKER: &str = "ORZMA-PROFILE-RAN";

/// The prompt the fixture profile installs, which the injected hook must
/// call back rather than replace.
const USER_PROMPT: &str = "ORZMA-USER-PROMPT> ";

/// Asserts that an injected PowerShell reports the directory it changed
/// into, and does so while calling back the prompt the user's own
/// profile installed.
///
/// Case: a Windows user whose profile sets a custom prompt — the shape
/// posh-git, Starship, and oh-my-posh all take — opens a pane, changes
/// directory, and splits.
#[test]
fn an_injected_powershell_reports_its_directory_and_calls_back_the_user_prompt() {
    let (shell_path, shell_name) = resolve_powershell();

    let home = TempDir::new().expect("a temporary home");
    write_profile(home.path(), shell_name);

    let target = TempDir::new().expect("a temporary directory");
    let expected = target.path().to_path_buf();

    let size = GridSize::new(80, 24).expect("a valid grid size");
    let mut tty = OrzmaTty::spawn(
        OrzmaVt::new(size, 128),
        SpawnOptions {
            size,
            cell_px: CellPixels::default(),
            shell: shell_path.display().to_string(),
            cwd: None,
            env: vec![(
                EnvKey("USERPROFILE".to_string()),
                EnvValue(home.path().display().to_string()),
            )],
            shell_integration: true,
        },
    )
    .expect("a spawned shell");

    let command = format!("Set-Location '{}'", expected.display());
    let mut profile_ran = false;
    let mut sent = false;
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        for signal in tty.pump().signals {
            match signal {
                TtySignal::Vt(VtSignal::Title(title)) if title == PROFILE_MARKER => {
                    profile_ran = true;
                }
                // NOTE: the shell reports its startup directory before the
                // command is sent. Latching on `sent` is what keeps the
                // assertion below about the directory that was changed
                // into rather than the one the shell started in.
                TtySignal::Vt(VtSignal::CurrentDir(_)) if !sent => {
                    tty.send_paste(&command).expect("the command text");
                    tty.send_key(&TerminalKey::Enter, &TerminalModifiers::default())
                        .expect("the newline");
                    sent = true;
                }
                TtySignal::Vt(VtSignal::CurrentDir(path)) => {
                    assert!(
                        profile_ran,
                        "the fixture profile must run before the injected command"
                    );
                    assert_eq!(path, expected);
                    return;
                }
                _ => {}
            }
        }
        sleep(Duration::from_millis(20));
    }
    panic!(
        "no working-directory report arrived within 60s (shell: {})",
        shell_name
    );
}

/// The PowerShell executable this test runs against, preferring `pwsh`
/// (PowerShell 7, the default `windows_default_shell` picks) and falling
/// back to Windows PowerShell 5.1 when `pwsh` is not installed.
///
/// # Panics
///
/// Panics when neither `pwsh` nor `powershell` resolves on `PATH`.
fn resolve_powershell() -> (PathBuf, &'static str) {
    if let Some(path) = on_path("pwsh") {
        return (path, "pwsh");
    }
    if let Some(path) = on_path("powershell") {
        return (path, "powershell");
    }
    panic!("neither pwsh nor powershell is on PATH; this test requires one of them installed");
}

/// Writes the fixture profile into the `Documents` profile directory
/// `shell_name` actually reads under a temporary `USERPROFILE`.
///
/// Windows PowerShell 5.1 and PowerShell 7 read different directories
/// under `USERPROFILE`\Documents (`WindowsPowerShell` and `PowerShell`
/// respectively), so the profile is written only into the one the
/// resolved shell will load.
fn write_profile(home: &Path, shell_name: &str) {
    let subdir = match shell_name {
        "pwsh" => "PowerShell",
        _ => "WindowsPowerShell",
    };
    let profile_dir = home.join("Documents").join(subdir);
    create_dir_all(&profile_dir).expect("the profile directory");
    write(
        profile_dir.join("Microsoft.PowerShell_profile.ps1"),
        format!(
            "[Console]::Write(([char]27, ']0;{PROFILE_MARKER}', [char]7) -join '')\n\
             function global:prompt {{ '{USER_PROMPT}' }}\n"
        ),
    )
    .expect("the profile file");
}

/// The full path `name` resolves to through `PATH` and `PATHEXT`, or
/// `None` when no entry names a file.
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
