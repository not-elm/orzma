use thiserror::Error;

pub type OzmuxResult<T = ()> = Result<T, OzmuxError>;

#[derive(Error, Debug, Clone)]
pub enum OzmuxError {
    #[error("failed to launch daemon http server:{0}")]
    FailedLaunchHttpServer(String),
}
