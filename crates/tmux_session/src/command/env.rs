//! Session-scoped environment commands used to propagate `$OZMA_SOCK` to panes
//! spawned after attach / switch.

use crate::input::quote;
use tmux_control::TmuxCommand;

/// `set-environment <key> <value>` — sets a var on the control client's current session.
pub struct SetEnvironment<'a> {
    /// Environment variable name.
    pub key: &'a str,
    /// Environment variable value.
    pub value: &'a str,
}
impl TmuxCommand for SetEnvironment<'_> {
    fn into_raw_command(self) -> String {
        format!("set-environment {} {}", quote(self.key), quote(self.value))
    }
}

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
    fn set_environment_is_session_scoped() {
        assert_eq!(
            SetEnvironment {
                key: "OZMA_SOCK",
                value: "/tmp/ctl.sock"
            }
            .into_raw_command(),
            "set-environment OZMA_SOCK /tmp/ctl.sock"
        );
    }

    #[test]
    fn set_environment_quotes_paths_with_spaces() {
        assert_eq!(
            SetEnvironment {
                key: "OZMA_SOCK",
                value: "/tmp/a b/ctl.sock"
            }
            .into_raw_command(),
            "set-environment OZMA_SOCK '/tmp/a b/ctl.sock'"
        );
    }

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
