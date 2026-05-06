use crate::error::ExtensionResult;
use crate::manifest::{CommandName, CommandScriptPath, ExtensionManifest};
use std::{collections::HashMap, path::Path};

#[derive(Debug, Clone)]
pub struct ExtensionCommands(HashMap<CommandName, CommandScriptPath>);

impl ExtensionCommands {
    /// Load all extensions from `OZMUX_EXTENSION_DIR`. Returns an
    /// empty set if the env var is unset (or set to non-UTF-8).
    pub async fn load() -> ExtensionResult<Self> {
        let root = std::env::var("OZMUX_EXTENSION_DIR").ok();
        Self::load_from(root.as_deref().map(std::path::Path::new)).await
    }

    /// Load extensions from a specific directory (or `None` = empty).
    /// Per-extension parse failures are skipped silently. Invalid
    /// command names and duplicates are warn-logged.
    pub async fn load_from(extension_root: Option<&std::path::Path>) -> ExtensionResult<Self> {
        let mut commands = HashMap::default();
        let Some(root) = extension_root else {
            return Ok(Self(commands));
        };
        for entry in std::fs::read_dir(root)?.filter_map(|r| r.ok()) {
            let Some(manifest) = load_manifest(&entry.path()) else {
                continue;
            };
            for (name, script) in manifest.commands {
                if !is_valid_command_name(name.as_ref()) {
                    tracing::warn!(name = %name, "extension command name has invalid characters; skipping");
                    continue;
                }
                if commands.insert(name.clone(), script).is_some() {
                    tracing::warn!(name = %name, "duplicate extension command name; later one overrides");
                }
            }
        }
        Ok(Self(commands))
    }

    /// Materialize the command set as PATH wrapper scripts in a fresh
    /// temp directory. Each wrapper is named `@<command>` and execs
    /// `node '<escaped_script>' "$@"`. Returns the `TempDir` so its
    /// `Drop` tears down the directory.
    pub fn materialize_wrappers(&self) -> ExtensionResult<tempfile::TempDir> {
        let dir = tempfile::TempDir::new()?;
        for (name, script) in &self.0 {
            let wrapper_path = dir.path().join(format!("@{name}"));
            std::fs::write(&wrapper_path, Self::wrapper_body(script))?;
            Self::set_executable(&wrapper_path)?;
        }
        Ok(dir)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&CommandName, &CommandScriptPath)> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn wrapper_body(script: &CommandScriptPath) -> String {
        format!(
            "#!/bin/sh\nexec node {} \"$@\"\n",
            sh_single_quote(&script.0.display().to_string()),
        )
    }

    fn set_executable(path: &std::path::Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(path)?.permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(path, perm)
    }

    #[cfg(test)]
    pub(crate) fn from_map(m: HashMap<CommandName, CommandScriptPath>) -> Self {
        Self(m)
    }
}

fn load_manifest(extension_dir: &Path) -> Option<ExtensionManifest> {
    let buff = std::fs::read_to_string(extension_dir.join("ozmux.json")).ok()?;
    serde_json::from_str(&buff).ok()
}

/// POSIX shell single-quote escape: wrap the string in `'...'` and
/// replace any internal `'` with `'\''` (close, escape, reopen).
/// Result is always safely interpretable as a single shell word.
fn sh_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Restrict command names to ASCII alphanumerics, `_`, and `-`. This
/// keeps wrapper filenames safe (no `/`, `..`, control chars) and
/// shell-friendly (no quoting needed when invoking).
fn is_valid_command_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{CommandName, CommandScriptPath};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_commands(entries: &[(&str, &str)]) -> ExtensionCommands {
        let mut map = HashMap::new();
        for (name, path) in entries {
            map.insert(
                CommandName::new(*name),
                CommandScriptPath(PathBuf::from(path)),
            );
        }
        ExtensionCommands::from_map(map)
    }

    #[test]
    fn materialize_creates_wrapper_per_command() {
        let commands = make_commands(&[
            ("settings", "/x/s.js"),
            ("open", "/x/o.js"),
        ]);
        let dir = commands.materialize_wrappers().unwrap();
        assert!(dir.path().join("@settings").exists());
        assert!(dir.path().join("@open").exists());
    }

    #[test]
    fn wrapper_script_invokes_node_with_quoted_script_and_argv() {
        let commands = make_commands(&[("hello", "/path/hello.js")]);
        let dir = commands.materialize_wrappers().unwrap();
        let body = std::fs::read_to_string(dir.path().join("@hello")).unwrap();
        assert!(body.starts_with("#!/bin/sh\n"));
        assert!(body.contains("exec node '/path/hello.js'"));
        assert!(body.contains("\"$@\""));
    }

    #[test]
    fn wrapper_script_escapes_paths_with_spaces() {
        let commands = make_commands(&[("x", "/p ath/x.js")]);
        let dir = commands.materialize_wrappers().unwrap();
        let body = std::fs::read_to_string(dir.path().join("@x")).unwrap();
        assert!(body.contains("'/p ath/x.js'"));
    }

    #[test]
    fn wrapper_script_escapes_paths_with_single_quotes() {
        let commands = make_commands(&[("x", "/p'q/x.js")]);
        let dir = commands.materialize_wrappers().unwrap();
        let body = std::fs::read_to_string(dir.path().join("@x")).unwrap();
        // /p'q/x.js → '/p'\''q/x.js'
        assert!(body.contains(r"'/p'\''q/x.js'"));
    }

    #[test]
    fn wrapper_files_are_executable() {
        use std::os::unix::fs::PermissionsExt;
        let commands = make_commands(&[("x", "/x.js")]);
        let dir = commands.materialize_wrappers().unwrap();
        let perms = std::fs::metadata(dir.path().join("@x")).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o755);
    }

    #[test]
    fn empty_commands_creates_empty_dir() {
        let commands = make_commands(&[]);
        let dir = commands.materialize_wrappers().unwrap();
        assert!(dir.path().is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn temp_dir_is_removed_when_dropped() {
        let commands = make_commands(&[]);
        let dir = commands.materialize_wrappers().unwrap();
        let path = dir.path().to_path_buf();
        assert!(path.exists());
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn sh_single_quote_wraps_simple_string() {
        assert_eq!(sh_single_quote("simple"), "'simple'");
    }

    #[test]
    fn sh_single_quote_handles_spaces() {
        assert_eq!(sh_single_quote("with space"), "'with space'");
    }

    #[test]
    fn sh_single_quote_handles_dollar_and_backtick() {
        assert_eq!(sh_single_quote("with$var"), "'with$var'");
        assert_eq!(sh_single_quote("with`backtick`"), "'with`backtick`'");
    }

    #[test]
    fn sh_single_quote_escapes_internal_single_quote() {
        // close quote, escaped quote, reopen quote: ' → '\''
        assert_eq!(sh_single_quote("with'quote"), r"'with'\''quote'");
    }

    #[test]
    fn sh_single_quote_handles_empty_string() {
        assert_eq!(sh_single_quote(""), "''");
    }

    #[test]
    fn is_valid_command_name_accepts_simple_lowercase() {
        assert!(is_valid_command_name("settings"));
    }

    #[test]
    fn is_valid_command_name_accepts_dash_underscore_digits_uppercase() {
        assert!(is_valid_command_name("open-tab"));
        assert!(is_valid_command_name("a_b_c"));
        assert!(is_valid_command_name("X1"));
    }

    #[test]
    fn is_valid_command_name_rejects_empty() {
        assert!(!is_valid_command_name(""));
    }

    #[test]
    fn is_valid_command_name_rejects_path_traversal_chars() {
        assert!(!is_valid_command_name("../etc"));
        assert!(!is_valid_command_name("a/b"));
    }

    #[test]
    fn is_valid_command_name_rejects_whitespace_and_specials() {
        assert!(!is_valid_command_name("a b"));
        assert!(!is_valid_command_name("hello!"));
        assert!(!is_valid_command_name("a@b"));
    }

    #[tokio::test]
    async fn load_from_returns_empty_when_dir_is_none() {
        let commands = ExtensionCommands::load_from(None).await.unwrap();
        assert!(commands.is_empty());
    }

    #[tokio::test]
    async fn load_from_reads_ozmux_json_files() {
        let temp = tempfile::tempdir().unwrap();
        let ext_dir = temp.path().join("memo");
        std::fs::create_dir(&ext_dir).unwrap();
        std::fs::write(
            ext_dir.join("ozmux.json"),
            r#"{"commands":{"settings":"/abs/settings.js"}}"#,
        ).unwrap();
        let commands = ExtensionCommands::load_from(Some(temp.path())).await.unwrap();
        assert_eq!(commands.iter().count(), 1);
    }

    #[tokio::test]
    async fn load_from_skips_invalid_command_names() {
        let temp = tempfile::tempdir().unwrap();
        let ext_dir = temp.path().join("evil");
        std::fs::create_dir(&ext_dir).unwrap();
        std::fs::write(
            ext_dir.join("ozmux.json"),
            r#"{"commands":{"../../etc/passwd":"/x.js","":"/y.js","good":"/z.js"}}"#,
        ).unwrap();
        let commands = ExtensionCommands::load_from(Some(temp.path())).await.unwrap();
        let names: Vec<String> = commands.iter().map(|(n, _)| n.as_ref().to_string()).collect();
        assert_eq!(names, vec!["good".to_string()]);
    }

    #[tokio::test]
    async fn load_from_handles_duplicate_command_names_with_one_winner() {
        let temp = tempfile::tempdir().unwrap();
        for ext in ["a", "b"] {
            let ext_dir = temp.path().join(ext);
            std::fs::create_dir(&ext_dir).unwrap();
            std::fs::write(
                ext_dir.join("ozmux.json"),
                r#"{"commands":{"shared":"/some.js"}}"#,
            ).unwrap();
        }
        let commands = ExtensionCommands::load_from(Some(temp.path())).await.unwrap();
        // 重複した name は 1 件にまとまる（warn ログは出るが値は last-write-wins）
        assert_eq!(commands.iter().count(), 1);
    }
}
