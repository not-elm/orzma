//! Errors for the extension host.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExtensionHostError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("env var missing: {0}")]
    EnvVar(#[from] std::env::VarError),

    #[error("manifest parse failed: {0}")]
    Manifest(String),
}

pub type ExtensionHostResult<T = ()> = Result<T, ExtensionHostError>;
