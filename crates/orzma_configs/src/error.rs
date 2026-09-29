//! The error type the config loader reports, and the result alias built on it.

use crate::shortcuts::{DuplicateChord, KeyChord};
use crate::vi_mode::DuplicateViModeKey;
use std::path::PathBuf;
use thiserror::Error;

/// A `Result` whose error is [`OrzmaConfigsError`].
pub type OrzmaConfigsResult<T = ()> = Result<T, OrzmaConfigsError>;

/// Every failure the config loader reports: resolving, reading, parsing, or
/// validating the config file.
#[derive(Debug, Error)]
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

    /// One or more `KeyChord` collisions among direct `[shortcuts]` bindings.
    /// Every collision is reported, not just the first.
    #[error("duplicate chord(s) among direct [shortcuts] bindings: {}", format_dupes(.0))]
    DuplicateChords(Vec<DuplicateChord>),

    /// One or more `KeyChord` collisions among leader-scoped (`<Leader>`)
    /// bindings. Every collision is reported, not just the first.
    #[error("duplicate chord(s) among <Leader> bindings: {}", format_dupes(.0))]
    DuplicatePrefixChords(Vec<DuplicateChord>),

    /// The same key is bound to more than one `[vi-mode]` action.
    #[error("duplicate key(s) among [vi-mode] bindings: {}", format_vi_mode_dupes(.0))]
    DuplicateViModeKeys(Vec<DuplicateViModeKey>),

    /// The configured leader chord duplicates a direct `[shortcuts]` binding's
    /// chord, which would leave that binding unreachable.
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

    /// A `[font].<face>.style` string did not parse to a known weight + slant.
    #[error("invalid font style {value:?} for the {face} face")]
    InvalidFontStyle {
        /// The face label (`normal` / `bold` / `italic` / `bold_italic` / `ui`).
        face: &'static str,
        /// The offending style string.
        value: String,
    },

    /// Neither `$XDG_CONFIG_HOME` nor a home directory could be resolved.
    #[error("could not determine config directory (no $XDG_CONFIG_HOME and no home dir)")]
    HomeDirNotFound,

    /// A `[shortcuts]` chord string that does not parse.
    #[error(transparent)]
    KeyChord(#[from] KeyChordParseError),

    /// A `[vi-mode]` key string that does not parse.
    #[error(transparent)]
    ViModeKey(#[from] ViModeKeyParseError),

    /// A `[font]` style string with a token that names no weight or slant.
    #[error(transparent)]
    FontStyleToken(#[from] InvalidFontStyleToken),
}

/// The reason a chord string does not parse.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeyChordParseError {
    /// Consecutive `+` or trailing `+` produced an empty token between separators.
    #[error("empty token in chord string (consecutive '+' or trailing '+')")]
    EmptyToken,
    /// A token that is neither a known modifier nor a known named key.
    #[error("unknown named key: {0:?}")]
    UnknownNamedKey(String),
    /// The same modifier bit was set twice. Catches both literal duplicates
    /// (`"Cmd+Cmd+S"`) and alias collisions (`"Cmd+Meta+S"`, both set `meta`).
    #[error("duplicate modifier {token:?} (normalized to {normalized_bit})")]
    DuplicateModifier {
        /// The offending token as it appeared in the input.
        token: String,
        /// Which `Modifiers` bit was already set.
        normalized_bit: &'static str,
    },
    /// More than one non-modifier token appeared in the chord.
    #[error("multiple key tokens in chord string")]
    MultipleKeyTokens,
}

/// The reason a vi-mode key string does not parse.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ViModeKeyParseError {
    /// Empty key token.
    #[error("empty vi-mode key")]
    Empty,
    /// A modifier other than `Ctrl` (`Cmd`/`Alt`/`Shift`/aliases).
    #[error(
        "modifier {0:?} is not allowed in [vi-mode] (only Ctrl+); express Shift via the character case"
    )]
    ForbiddenModifier(String),
    /// More than one `+`-separated segment beyond `Ctrl+<key>`.
    #[error("too many tokens in vi-mode key {0:?} (expected [Ctrl+]<key>)")]
    TooManyTokens(String),
    /// A multi-character token that is not a known named key.
    #[error("unknown vi-mode key {0:?} (expected one character or a named key)")]
    UnknownKey(String),
    /// `Ctrl+` with a non-alphanumeric character.
    #[error("Ctrl+{0:?} is not allowed (Ctrl accepts ASCII alphanumerics and named keys only)")]
    CtrlNonAlphanumeric(String),
}

/// A `style` string that contained a token matching neither a weight nor a
/// slant name.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("unknown font style token {token:?}")]
pub struct InvalidFontStyleToken {
    /// The offending token, as written by the user.
    pub token: String,
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
