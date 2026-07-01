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

    /// One or more KeyChord collisions across the `[shortcuts.bindings]` table.
    /// Collected in a single pass; reported all-at-once to avoid whack-a-mole.
    #[error("duplicate chord(s) in shortcuts.bindings: {}", format_dupes(.0))]
    DuplicateChords(Vec<crate::shortcuts::DuplicateChord>),

    /// One or more KeyChord collisions among leader-scoped (`<Leader>`)
    /// bindings. Collected in a single pass; reported all-at-once.
    #[error("duplicate chord(s) among <Leader> bindings: {}", format_dupes(.0))]
    DuplicatePrefixChords(Vec<crate::shortcuts::DuplicateChord>),

    /// A `<Leader>`-scoped binding is set but no `[shortcuts] leader` is
    /// configured, so it is unreachable.
    #[error("a <Leader> binding is set but no [shortcuts] leader is configured")]
    PrefixBindingsWithoutLeader,

    /// The configured leader chord duplicates a direct `[shortcuts.bindings]`
    /// chord. The leader is matched first, so that direct binding would be
    /// unreachable.
    #[error("leader chord {chord} shadows the direct binding for {action}")]
    LeaderShadowsDirectBinding {
        /// The colliding chord (the leader).
        chord: crate::shortcuts::KeyChord,
        /// The direct-binding action label it shadows.
        action: &'static str,
    },

    /// The configured leader chord's logical key has no physical `KeyCode`
    /// mapping, so the whole `[shortcuts.prefix_bindings]` table would be
    /// silently unreachable.
    #[error(
        "leader chord {chord} has no physical key mapping; shortcuts.prefix_bindings would be unreachable"
    )]
    UnmappableLeader {
        /// The unmappable leader chord.
        chord: crate::shortcuts::KeyChord,
    },

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
