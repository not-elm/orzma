//! Error type for orzma config loading.

use crate::shortcuts::{DuplicateChord, KeyChord};
use crate::vi_mode::DuplicateViModeKey;
use std::path::PathBuf;

/// Result alias used throughout the `orzma_configs` crate.
pub type OrzmaConfigsResult<T = ()> = Result<T, OrzmaConfigsError>;

/// Errors that can occur while resolving, reading, or parsing the config file.
#[derive(Debug, thiserror::Error)]
pub enum OrzmaConfigsError {
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

    /// One or more KeyChord collisions among direct `[shortcuts]` bindings.
    /// Collected in a single pass; reported all-at-once to avoid whack-a-mole.
    #[error("duplicate chord(s) among direct [shortcuts] bindings: {}", format_dupes(.0))]
    DuplicateChords(Vec<DuplicateChord>),

    /// One or more KeyChord collisions among leader-scoped (`<Leader>`)
    /// bindings. Collected in a single pass; reported all-at-once.
    #[error("duplicate chord(s) among <Leader> bindings: {}", format_dupes(.0))]
    DuplicatePrefixChords(Vec<DuplicateChord>),

    /// The same key is bound to more than one `[vi-mode]` action.
    #[error("duplicate key(s) among [vi-mode] bindings: {}", format_vi_mode_dupes(.0))]
    DuplicateViModeKeys(Vec<DuplicateViModeKey>),

    /// The configured leader chord duplicates a direct `[shortcuts]` binding's
    /// chord. The leader is matched first, so that direct binding would be
    /// unreachable.
    #[error("leader chord {chord} shadows the direct binding for {action}")]
    LeaderShadowsDirectBinding {
        /// The colliding chord (the leader).
        chord: KeyChord,
        /// The direct-binding action label it shadows.
        action: &'static str,
    },

    /// The configured leader chord's logical key has no physical `KeyCode`
    /// mapping, so its `<Leader>` bindings would be silently unreachable.
    #[error(
        "leader chord {chord} has no physical key mapping; its <Leader> bindings would be unreachable"
    )]
    UnmappableLeader {
        /// The unmappable leader chord.
        chord: KeyChord,
    },

    /// The configured font size is outside the supported range.
    #[error("font size {size} is out of range (expected 0 < size <= 200)")]
    InvalidFontSize {
        /// The offending size value.
        size: f32,
    },

    /// A `[font]` face's `style` string contained an unrecognized token.
    #[error("invalid font style {value:?} for the {face} face")]
    InvalidFontStyle {
        /// The face label (`normal` / `bold` / `italic` / `bold_italic`).
        face: &'static str,
        /// The offending style string.
        value: String,
    },

    /// Neither `$XDG_CONFIG_HOME` nor a home directory could be resolved.
    #[error("could not determine config directory (no $XDG_CONFIG_HOME and no home dir)")]
    HomeDirNotFound,
}

fn format_dupes(dupes: &[DuplicateChord]) -> String {
    dupes
        .iter()
        .map(|d| format!("{} = [{}]", d.chord, d.actions.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

fn format_vi_mode_dupes(dupes: &[DuplicateViModeKey]) -> String {
    dupes
        .iter()
        .map(|d| format!("{} -> [{}]", d.key, d.actions.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_dir_not_found_display() {
        let err = OrzmaConfigsError::HomeDirNotFound;
        assert_eq!(
            err.to_string(),
            "could not determine config directory (no $XDG_CONFIG_HOME and no home dir)"
        );
    }

    #[test]
    fn invalid_font_style_display() {
        let err = OrzmaConfigsError::InvalidFontStyle {
            face: "italic",
            value: "Blod".into(),
        };
        assert_eq!(
            err.to_string(),
            "invalid font style \"Blod\" for the italic face"
        );
    }
}
