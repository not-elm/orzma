//! End-to-end cover for the working-directory report orzma injects into
//! PowerShell on Windows.

#![cfg(windows)]

use orzma_tty::prelude::{OrzmaTty, TerminalKey, TerminalModifiers, TtySignal};
use orzma_tty::{CellPixels, EnvKey, EnvValue, SpawnOptions};
use orzma_vt::prelude::{GridSize, OrzmaVt, VtSignal};
use std::ffi::OsString;
use std::fs::{create_dir_all, read_to_string, write};
use std::path::{Path, PathBuf};
use std::process::Command;
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

/// The title marker the `$?`-reading fixture profile emits when it reads
/// `$?` as `$true`.
const DOLLAR_QUESTION_OK: &str = "ORZMA-DOLLARQ-OK";

/// The title marker the `$?`-reading fixture profile emits when it reads
/// `$?` as `$false`.
const DOLLAR_QUESTION_FAIL: &str = "ORZMA-DOLLARQ-FAIL";

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
    if !write_profile(home.path(), &shell_path) {
        eprintln!(
            "skipped: {shell_name} resolves its profile outside the temporary home, so this \
             machine's Documents folder is redirected and no fixture profile can be isolated"
        );
        return;
    }

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
        let signals: Vec<TtySignal> = tty.pump().signals().cloned().collect();
        for signal in signals {
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

/// Asserts that a command that fails still reaches the user's prompt as
/// failed, through the injected hook.
///
/// Case: a Windows user whose profile reads `$?` to color its prompt —
/// the shape Starship, posh-git, and oh-my-posh all take — runs a
/// command that fails.
#[test]
fn a_failing_command_still_shows_as_failed_through_the_injected_hook() {
    let (shell_path, shell_name) = resolve_powershell();

    let home = TempDir::new().expect("a temporary home");
    if !write_dollar_question_profile(home.path(), &shell_path) {
        eprintln!(
            "skipped: {shell_name} resolves its profile outside the temporary home, so this \
             machine's Documents folder is redirected and no fixture profile can be isolated"
        );
        return;
    }

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

    let command = r"Get-Item Z:\orzma-fixture-path-that-does-not-exist";
    let mut sent = false;
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        let signals: Vec<TtySignal> = tty.pump().signals().cloned().collect();
        for signal in signals {
            match signal {
                // NOTE: the first marker the shell renders reflects its own
                // startup state, not the fixture command below, so it only
                // marks the profile as loaded and the hook as active.
                TtySignal::Vt(VtSignal::Title(title))
                    if !sent && (title == DOLLAR_QUESTION_OK || title == DOLLAR_QUESTION_FAIL) =>
                {
                    tty.send_paste(command).expect("the command text");
                    tty.send_key(&TerminalKey::Enter, &TerminalModifiers::default())
                        .expect("the newline");
                    sent = true;
                }
                TtySignal::Vt(VtSignal::Title(title)) if sent && title == DOLLAR_QUESTION_FAIL => {
                    return;
                }
                _ => {}
            }
        }
        sleep(Duration::from_millis(20));
    }
    panic!(
        "no ORZMA-DOLLARQ-FAIL title arrived within 60s (shell: {})",
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

/// Writes the fixture profile that announces itself through an `OSC 0`
/// marker and installs a custom prompt, or reports `false` when `shell`
/// resolves its profile outside `home`.
fn write_profile(home: &Path, shell: &Path) -> bool {
    write_profile_contents(
        home,
        shell,
        format!(
            "[Console]::Write(([char]27, ']0;{PROFILE_MARKER}', [char]7) -join '')\n\
             function global:prompt {{ '{USER_PROMPT}' }}\n"
        ),
    )
}

/// Writes a fixture profile whose prompt reads `$?` as its own first
/// statement and reports what it saw through an `OSC 0` title, or
/// reports `false` when `shell` resolves its profile outside `home`.
///
/// The shape mirrors Starship, posh-git, and oh-my-posh, which all read
/// `$?` before anything else in their own prompt function.
fn write_dollar_question_profile(home: &Path, shell: &Path) -> bool {
    write_profile_contents(
        home,
        shell,
        format!(
            "function global:prompt {{ \
             $orzmaSawSuccess = $?; \
             if ($orzmaSawSuccess) {{ [Console]::Write(([char]27, ']0;{DOLLAR_QUESTION_OK}', [char]7) -join '') }} \
             else {{ [Console]::Write(([char]27, ']0;{DOLLAR_QUESTION_FAIL}', [char]7) -join '') }}; \
             return '{USER_PROMPT}' \
             }}\n"
        ),
    )
}

/// Writes `contents` into the file `shell` itself reports as its
/// per-user profile under a temporary `USERPROFILE`, and reports whether
/// that file landed inside `home`.
///
/// A machine whose Documents folder is redirected — by OneDrive's Known
/// Folder Move, for instance — resolves the profile outside `home`, and
/// nothing is written there.
fn write_profile_contents(home: &Path, shell: &Path, contents: String) -> bool {
    let Some(profile) = resolve_profile_path(home, shell) else {
        return false;
    };
    let Some(profile_dir) = profile.parent() else {
        return false;
    };
    create_dir_all(profile_dir).expect("the profile directory");
    write(&profile, contents).expect("the profile file");
    true
}

/// The absolute path `shell` reports as its per-user profile under a
/// temporary `USERPROFILE`, or `None` when it resolves outside `home`.
///
/// Windows PowerShell 5.1 and PowerShell 7 read different directories
/// under Documents, and Documents is a known folder rather than a plain
/// `USERPROFILE` subdirectory, so the shell is asked rather than
/// guessed. A machine whose Documents folder is redirected — by
/// OneDrive's Known Folder Move, for instance — resolves the profile
/// outside `home`, which this reports as `None`.
fn resolve_profile_path(home: &Path, shell: &Path) -> Option<PathBuf> {
    // NOTE: the Documents known folder resolves to the empty string when
    // its directory does not exist, which would leave the reported
    // profile path relative. It has to exist before the shell is asked.
    create_dir_all(home.join("Documents")).expect("the Documents directory");
    // NOTE: a redirected stdout carries the console code page, which
    // mangles a non-ASCII user name in the path. `WriteAllText` is UTF-8,
    // and passing its destination through the environment keeps the
    // snippet free of a path this test would have to quote.
    let answer = home.join("reported-profile-path");
    Command::new(shell)
        .args([
            "-NoProfile",
            "-NoLogo",
            "-Command",
            "[IO.File]::WriteAllText($env:ORZMA_PROFILE_ANSWER, $PROFILE.CurrentUserCurrentHost)",
        ])
        .env("USERPROFILE", home)
        .env("ORZMA_PROFILE_ANSWER", &answer)
        .status()
        .expect("the shell reports its profile path");
    let answered = read_to_string(&answer).expect("the shell's reported profile path");
    let profile = PathBuf::from(answered.trim());
    profile.starts_with(home).then_some(profile)
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
