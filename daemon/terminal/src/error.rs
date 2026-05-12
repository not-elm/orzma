//! Domain errors for the terminal layer.

use ozmux_multiplexer::ActivityId;
use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum TerminalError {
    #[error("activity not found activity-id={0}")]
    ActivityNotFound(ActivityId),

    #[error("failed pty: {0}")]
    Pty(String),
}

pub type TerminalResult<T = ()> = Result<T, TerminalError>;

pub trait PtyErrorBridge<T> {
    fn to_terminal_result(self) -> TerminalResult<T>;
}

impl<T> PtyErrorBridge<T> for anyhow::Result<T> {
    fn to_terminal_result(self) -> TerminalResult<T> {
        match self {
            Ok(t) => Ok(t),
            Err(e) => Err(TerminalError::Pty(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_error_displays_with_failed_pty_prefix() {
        let err = TerminalError::Pty("oops".into());
        assert_eq!(err.to_string(), "failed pty: oops");
    }

    #[test]
    fn anyhow_err_converts_to_terminal_pty() {
        let r: anyhow::Result<()> = Err(anyhow::anyhow!("boom"));
        let out = r.to_terminal_result();
        match out {
            Err(TerminalError::Pty(msg)) => assert!(msg.contains("boom")),
            _ => panic!("expected TerminalError::Pty"),
        }
    }
}
