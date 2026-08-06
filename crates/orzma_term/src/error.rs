pub type OrzmaTermResult<T = ()> = Result<T, OrzmaTermError>;

#[derive(Debug, thiserror::Error)]
pub enum OrzmaTermError {}
