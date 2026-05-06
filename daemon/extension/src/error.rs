use thiserror::Error;

pub type ExtensionResult<T = ()> = Result<T, ExtensionError>;

#[derive(Error, Debug)]
pub enum ExtensionError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("manifest parse failed: {0}")]
    Manifest(String),
}
