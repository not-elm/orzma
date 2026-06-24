//! Error type for ozmux config loading.

use std::path::PathBuf;

/// Result alias used throughout the `ozmux_configs` crate.
pub type OzmuxConfigsResult<T = ()> = Result<T, OzmuxConfigsError>;

/// Errors that can occur while resolving, reading, or parsing the config file.
#[derive(Debug, thiserror::Error)]
pub enum OzmuxConfigsError {
    /// Reading the config file failed for a reason other than `NotFound`.
    #[error("failed to read config file at {path}")]
    Io {
        /// Path that was being read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The config file contains invalid TOML.
    #[error("failed to parse TOML at {path}")]
    ParseToml {
        /// Path of the offending file.
        path: PathBuf,
        /// Underlying parser error.
        #[source]
        source: toml::de::Error,
    },

    /// One or more KeyChord collisions across `[shortcuts.bindings]` and
    /// `[shortcuts.commands]`. Collected in a single pass; reported all-at-once.
    #[error("duplicate chord(s) in shortcuts.bindings/commands: {}", format_dupes(.0))]
    DuplicateChords(Vec<crate::shortcuts::DuplicateChord>),

    /// The configured font size is outside the supported range.
    #[error("font size {size} is out of range (expected 0 < size <= 200)")]
    InvalidFontSize {
        /// The offending size value.
        size: f32,
    },

    /// Neither `$XDG_CONFIG_HOME` nor a home directory could be resolved.
    #[error("could not determine config directory (no $XDG_CONFIG_HOME and no home dir)")]
    HomeDirNotFound,
}

fn format_dupes(dupes: &[crate::shortcuts::DuplicateChord]) -> String {
    dupes
        .iter()
        .map(|d| format!("{} = [{}]", d.chord, d.actions.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_dir_not_found_display() {
        let err = OzmuxConfigsError::HomeDirNotFound;
        assert_eq!(
            err.to_string(),
            "could not determine config directory (no $XDG_CONFIG_HOME and no home dir)"
        );
    }
}
