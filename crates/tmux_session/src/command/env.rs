//! Session-scoped environment commands used to propagate `$OZMA_SOCK` to panes
//! spawned after attach / switch.

use crate::input::quote;
use tmux_control::TmuxCommand;

/// `set-environment -t <session> <key> <value>` — sets a var on a specific session.
pub struct SetEnvironmentInSession<'a> {
    /// Target session name.
    pub session: &'a str,
    /// Environment variable name.
    pub key: &'a str,
    /// Environment variable value.
    pub value: &'a str,
}
impl TmuxCommand for SetEnvironmentInSession<'_> {
    fn into_raw_command(self) -> String {
        format!(
            "set-environment -t {} {} {}",
            quote(self.session),
            quote(self.key),
            quote(self.value)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_environment_in_session_targets_named_session() {
        assert_eq!(
            SetEnvironmentInSession {
                session: "work",
                key: "OZMA_SOCK",
                value: "/tmp/ctl.sock"
            }
            .into_raw_command(),
            "set-environment -t work OZMA_SOCK /tmp/ctl.sock"
        );
    }

    #[test]
    fn set_environment_in_session_quotes_session_and_value() {
        assert_eq!(
            SetEnvironmentInSession {
                session: "my work",
                key: "OZMA_SOCK",
                value: "/tmp/a b/ctl.sock"
            }
            .into_raw_command(),
            "set-environment -t 'my work' OZMA_SOCK '/tmp/a b/ctl.sock'"
        );
    }
}
