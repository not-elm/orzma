use crate::error::ExtensionResult;
use crate::manifest::{CommandName, CommandScriptPath, ExtensionManifest};
use std::{collections::HashMap, path::Path};

#[derive(Debug, Clone)]
pub struct ExtensionCommands(
    // TODO: remove allow once Task 3 (materialize_wrappers) reads the field.
    #[allow(dead_code)] HashMap<CommandName, CommandScriptPath>,
);

impl ExtensionCommands {
    pub async fn load() -> ExtensionResult<Self> {
        let mut commands = HashMap::default();
        let extension_root = match std::env::var("OZMUX_EXTENSION_DIR") {
            Ok(root) => root,
            Err(_) => return Ok(Self(commands)), // Missing env var is not an error
        };
        for entry in std::fs::read_dir(&extension_root)?.filter_map(|r| r.ok()) {
            if let Some(manifest) = load_manifest(&entry.path()) {
                commands.extend(manifest.commands);
            }
        }
        Ok(Self(commands))
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
}
