//! Error types for the extension host crate.

/// A failure originating in the extension host (e.g. manifest parsing).
#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    /// Malformed or invalid `ozmux.toml`.
    #[error("invalid ozmux.toml: {0}")]
    Toml(#[source] toml::de::Error),
}
