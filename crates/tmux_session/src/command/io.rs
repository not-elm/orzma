//! Pane-input commands: inject raw bytes, or deliver named keys to a pane.

use crate::input::quote;
use std::fmt::Write;
use tmux_control::TmuxCommand;

/// `send-keys -H -t <pane> <hex>…` — injects raw bytes into a pane (terminal replies).
pub struct SendBytes<'a> {
    /// The tmux pane target (e.g. `%3`).
    pub pane: &'a str,
    /// Raw bytes to inject.
    pub bytes: &'a [u8],
}

impl TmuxCommand for SendBytes<'_> {
    fn into_raw_command(self) -> String {
        let mut cmd = format!("send-keys -H -t {}", quote(self.pane));
        for b in self.bytes {
            let _ = write!(cmd, " {b:02x}");
        }
        cmd
    }
}

/// `send-keys -t <pane> -- <name>…` — delivers named keys straight to a pane.
///
/// This is the forward path, NOT `send-keys -K`. Under `tmux -CC`, `-K`
/// mis-encodes named keys (e.g. `Up` arrives as a literal `n`), so keys go
/// directly to the pane, whose input encoder translates them correctly.
pub struct SendPaneKeys<'a> {
    /// The tmux pane target (e.g. `%3`).
    pub pane: &'a str,
    /// Tmux key names to deliver (e.g. `["a", "C-c", "Up"]`).
    pub names: &'a [String],
}

impl TmuxCommand for SendPaneKeys<'_> {
    fn into_raw_command(self) -> String {
        let mut cmd = format!("send-keys -t {} --", quote(self.pane));
        for n in self.names {
            cmd.push(' ');
            cmd.push_str(&quote(n));
        }
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_bytes_emits_hex_pairs() {
        assert_eq!(
            SendBytes {
                pane: "%3",
                bytes: &[0x1b, b'[', b'0', b'n']
            }
            .into_raw_command(),
            "send-keys -H -t %3 1b 5b 30 6e"
        );
    }

    #[test]
    fn send_pane_keys_quotes_each_name() {
        assert_eq!(
            SendPaneKeys {
                pane: "%3",
                names: &["a".into(), "C-c".into(), "Up".into()]
            }
            .into_raw_command(),
            "send-keys -t %3 -- a C-c Up"
        );
        assert_eq!(
            SendPaneKeys {
                pane: "%1",
                names: &["-".into()]
            }
            .into_raw_command(),
            "send-keys -t %1 -- -"
        );
        assert_eq!(
            SendPaneKeys {
                pane: "%2",
                names: &[";".into()]
            }
            .into_raw_command(),
            "send-keys -t %2 -- ';'"
        );
    }
}
