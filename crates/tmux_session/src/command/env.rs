//! Session-scoped and global environment commands used to propagate `$OZMA_SOCK`
//! to panes spawned after attach / switch.

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

/// `set-environment -g <key> <value>` — sets a var in the tmux global environment
/// so all sessions (existing and future) inherit it.
pub struct SetEnvironmentGlobal<'a> {
    /// Environment variable name.
    pub key: &'a str,
    /// Environment variable value.
    pub value: &'a str,
}
impl TmuxCommand for SetEnvironmentGlobal<'_> {
    fn into_raw_command(self) -> String {
        format!(
            "set-environment -g {} {}",
            quote(self.key),
            quote(self.value)
        )
    }
}

/// `set-environment -gu <key>` — removes a var from the tmux global environment.
pub struct UnsetEnvironmentGlobal<'a> {
    /// Environment variable name to unset.
    pub key: &'a str,
}
impl TmuxCommand for UnsetEnvironmentGlobal<'_> {
    fn into_raw_command(self) -> String {
        format!("set-environment -gu {}", quote(self.key))
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

    #[test]
    fn set_environment_global_emits_dash_g_flag() {
        assert_eq!(
            SetEnvironmentGlobal {
                key: "OZMA_SOCK",
                value: "/tmp/ctl.sock"
            }
            .into_raw_command(),
            "set-environment -g OZMA_SOCK /tmp/ctl.sock"
        );
    }

    #[test]
    fn set_environment_global_quotes_value_with_spaces() {
        assert_eq!(
            SetEnvironmentGlobal {
                key: "OZMA_SOCK",
                value: "/tmp/a b/ctl.sock"
            }
            .into_raw_command(),
            "set-environment -g OZMA_SOCK '/tmp/a b/ctl.sock'"
        );
    }

    #[test]
    fn unset_environment_global_emits_dash_gu_flag() {
        assert_eq!(
            UnsetEnvironmentGlobal { key: "OZMA_SOCK" }.into_raw_command(),
            "set-environment -gu OZMA_SOCK"
        );
    }
}
