//! Builds the `PATH` prefix that puts extension command shims on a terminal's
//! `PATH`, pinning `__builtin` first so built-in shims shadow extension commands.

use std::path::{Path, PathBuf};

const BUILTIN_DIR_NAME: &str = "__builtin";

/// Prepends `bin_dirs` to `existing_path` (`__builtin` first, rest sorted,
/// `:`-joined). Returns `existing_path` unchanged when `bin_dirs` is empty.
pub fn extension_path_prefix(bin_dirs: &[PathBuf], existing_path: &str) -> String {
    if bin_dirs.is_empty() {
        return existing_path.to_string();
    }
    let (mut builtin, mut rest): (Vec<String>, Vec<String>) = bin_dirs
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .partition(|p| Path::new(p).file_name().and_then(|n| n.to_str()) == Some(BUILTIN_DIR_NAME));
    rest.sort();
    builtin.append(&mut rest);
    let prefix = builtin.join(":");
    if existing_path.is_empty() {
        prefix
    } else {
        format!("{prefix}:{existing_path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_existing_unchanged() {
        assert_eq!(extension_path_prefix(&[], "/usr/bin"), "/usr/bin");
    }

    #[test]
    fn single_dir_is_prepended() {
        let dirs = vec![PathBuf::from("/run/ozmux/bin")];
        assert_eq!(
            extension_path_prefix(&dirs, "/usr/bin"),
            "/run/ozmux/bin:/usr/bin"
        );
    }

    #[test]
    fn builtin_pinned_first_rest_sorted() {
        let dirs = vec![
            PathBuf::from("/r/zeta"),
            PathBuf::from("/r/__builtin"),
            PathBuf::from("/r/alpha"),
        ];
        assert_eq!(
            extension_path_prefix(&dirs, "/usr/bin"),
            "/r/__builtin:/r/alpha:/r/zeta:/usr/bin"
        );
    }

    #[test]
    fn empty_existing_path_yields_just_prefix() {
        let dirs = vec![PathBuf::from("/run/ozmux/bin")];
        assert_eq!(extension_path_prefix(&dirs, ""), "/run/ozmux/bin");
    }
}
