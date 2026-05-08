use thiserror::Error;

pub type ExtensionResult<T = ()> = Result<T, ExtensionError>;

#[derive(Error, Debug)]
pub enum ExtensionError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialize error: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("mission env: {0}")]
    MissingEnv(String),
}
