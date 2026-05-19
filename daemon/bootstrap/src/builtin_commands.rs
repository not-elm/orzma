//! Built-in `@<name>` command registry materialized as PATH shims.
//!
//! Each entry in `BUILTINS` becomes one `#!/bin/sh` script under
//! `runtime_root/bin/__builtin/`, which `exec`s the running `ozmux`
//! binary with a fixed argv prefix. The shims are owner-only (0500),
//! the directory is owner-only (0700), and the whole tree is removed
//! by `RuntimeRoot::Drop`.
//!
//! See `docs/superpowers/specs/2026-05-19-builtin-command-aliases-design.md`.

use anyhow::{Context, Result, bail};
use std::path::Path;

/// Name of the subdirectory under `runtime_root/bin/` that holds
/// built-in shims. The name is reserved: an extension whose
/// `package.json.name` equals this string will trigger the
/// reserved-name pre-pass in `daemon/bootstrap/src/lib.rs`.
pub(crate) const BUILTIN_DIR_NAME: &str = "__builtin";

/// One built-in command. `shim_name` is the file the shell will look
/// up on `PATH`; `cli_args` are the arguments passed to the `ozmux`
/// binary after the shim's user-supplied args.
pub(crate) struct BuiltinCommand {
    pub shim_name: &'static str,
    pub cli_args: &'static [&'static str],
}

/// The registry of built-in commands shipped with the daemon. Add new
/// entries here; no other code changes are required.
pub(crate) const BUILTINS: &[BuiltinCommand] = &[BuiltinCommand {
    shim_name: "@browser",
    cli_args: &["browser"],
}];

/// Creates `bin_dir` (0700) and writes one shim file (0500) per entry
/// in `BUILTINS`. Per-shim failures are reported via the returned
/// `Result::Err` for the first failure; callers should wrap with
/// `tracing::error!` and continue (the daemon does not fail because
/// of a missing built-in).
pub(crate) async fn materialize(bin_dir: &Path, ozmux_exe: &Path) -> Result<()> {
    tokio::fs::create_dir_all(bin_dir)
        .await
        .with_context(|| format!("create built-in bin dir {}", bin_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(bin_dir, std::fs::Permissions::from_mode(0o700))
            .await
            .with_context(|| format!("chmod 0700 {}", bin_dir.display()))?;
    }

    for cmd in BUILTINS {
        let path = bin_dir.join(cmd.shim_name);
        let body = render_shim(ozmux_exe, cmd.cli_args);
        write_shim_file(&path, &body)
            .await
            .with_context(|| format!("write built-in shim {}", path.display()))?;
    }
    Ok(())
}

/// Verifies that the absolute path that will be baked into every
/// shim is not itself inside the runtime bin tree. Pure — no
/// filesystem access. Defends against the pyenv-#2696 class of
/// self-recursion bugs where `current_exe()` resolves to a shim.
pub(crate) fn validate_ozmux_exe(runtime_bin_dir: &Path, ozmux_exe: &Path) -> Result<()> {
    if ozmux_exe.starts_with(runtime_bin_dir) {
        bail!(
            "ozmux executable path {} is inside the runtime bin dir {} — refusing to bake a self-recursive shim",
            ozmux_exe.display(),
            runtime_bin_dir.display(),
        );
    }
    Ok(())
}

/// Quotes a string for embedding inside POSIX `sh` single quotes.
/// Mirrors the SDK's `shellSingleQuote` helper in
/// `sdk/typescript/src/server/shim-writer.ts`.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Renders a shim script for one built-in command.
fn render_shim(ozmux_exe: &Path, cli_args: &[&str]) -> String {
    let exe = shell_single_quote(&ozmux_exe.to_string_lossy());
    let args = cli_args
        .iter()
        .map(|a| shell_single_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    format!("#!/bin/sh\nexec {exe} {args} \"$@\"\n")
}

async fn write_shim_file(path: &Path, body: &str) -> Result<()> {
    // NOTE: best-effort remove first so writing to an existing 0500 file
    // (which the user cannot overwrite) cannot block us.
    let _ = tokio::fs::remove_file(path).await;
    tokio::fs::write(path, body)
        .await
        .with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o500))
            .await
            .with_context(|| format!("chmod 0500 {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn validate_ozmux_exe_accepts_path_outside_bin_dir() {
        let bin = Path::new("/tmp/ozmux/123/bin");
        let exe = Path::new("/usr/local/bin/ozmux");
        assert!(validate_ozmux_exe(bin, exe).is_ok());
    }

    #[test]
    fn validate_ozmux_exe_rejects_path_equal_to_bin_dir() {
        let bin = Path::new("/tmp/ozmux/123/bin");
        let exe = Path::new("/tmp/ozmux/123/bin");
        assert!(validate_ozmux_exe(bin, exe).is_err());
    }

    #[test]
    fn validate_ozmux_exe_rejects_path_under_bin_dir() {
        let bin = Path::new("/tmp/ozmux/123/bin");
        let exe = Path::new("/tmp/ozmux/123/bin/__builtin/@browser");
        assert!(validate_ozmux_exe(bin, exe).is_err());
    }

    #[test]
    fn shell_single_quote_wraps_plain_strings() {
        assert_eq!(shell_single_quote("/usr/bin/ozmux"), "'/usr/bin/ozmux'");
    }

    #[test]
    fn shell_single_quote_escapes_embedded_apostrophes() {
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn render_shim_emits_expected_lines() {
        let body = render_shim(Path::new("/usr/bin/ozmux"), &["browser"]);
        assert_eq!(body, "#!/bin/sh\nexec '/usr/bin/ozmux' 'browser' \"$@\"\n");
    }

    #[tokio::test]
    async fn materialize_creates_dir_and_shims_with_correct_modes() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("__builtin");
        let ozmux = Path::new("/usr/bin/true");
        materialize(&bin, ozmux).await.expect("materialize ok");

        // Directory must exist and be 0700.
        let dir_meta = std::fs::metadata(&bin).expect("bin dir exists");
        assert!(dir_meta.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(dir_meta.permissions().mode() & 0o777, 0o700);
        }

        // One shim per entry, each 0500.
        for cmd in BUILTINS {
            let shim = bin.join(cmd.shim_name);
            let meta = std::fs::metadata(&shim).expect("shim exists");
            assert!(meta.is_file());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(meta.permissions().mode() & 0o777, 0o500);
            }

            // Content sanity: header + the absolute exec path single-quoted.
            let body = std::fs::read_to_string(&shim).expect("shim readable");
            assert!(body.starts_with("#!/bin/sh\n"), "shim header: {body:?}");
            assert!(
                body.contains("'/usr/bin/true'"),
                "shim must reference baked exe: {body:?}"
            );

            // POSIX sh syntactic validity.
            let status = Command::new("sh").arg("-n").arg(&shim).status().unwrap();
            assert!(status.success(), "sh -n failed for {}", shim.display());
        }
    }

    #[tokio::test]
    async fn materialize_is_idempotent_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("__builtin");
        materialize(&bin, Path::new("/usr/bin/true")).await.unwrap();
        materialize(&bin, Path::new("/usr/bin/false"))
            .await
            .unwrap();

        for cmd in BUILTINS {
            let body = std::fs::read_to_string(bin.join(cmd.shim_name)).unwrap();
            assert!(
                body.contains("'/usr/bin/false'"),
                "second materialize should have overwritten: {body:?}"
            );
        }
    }
}
