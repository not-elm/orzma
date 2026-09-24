//! Locates the PowerShell executable the Windows end-to-end tests run
//! against.

use std::env;
use std::ffi::OsString;
use std::iter;
use std::path::{Path, PathBuf};

/// The PowerShell executable a test runs against and the name it resolved
/// under, preferring `pwsh` (PowerShell 7, the default
/// `windows_default_shell` picks) and falling back to Windows PowerShell
/// 5.1 when `pwsh` is not installed.
///
/// # Panics
///
/// Panics when neither `pwsh` nor `powershell` resolves on `PATH`.
pub fn resolve_powershell() -> (PathBuf, &'static str) {
    if let Some(path) = on_path("pwsh") {
        return (path, "pwsh");
    }
    if let Some(path) = on_path("powershell") {
        return (path, "powershell");
    }
    panic!("neither pwsh nor powershell is on PATH; this test requires one of them installed");
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
