//! Neutral control events produced by the `Vt` engine (Bevy-free).
//!
//! The Bevy layer translates each `VtEvent` into the matching
//! `EntityEvent` (`TerminalBell` / `TerminalTitleChanged` / …).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A control event raised by the terminal. The Bevy layer translates
/// it into an `EntityEvent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VtEvent {
    /// Bell (BEL).
    Bell,
    /// Window title change. `None` is an OSC 2 reset.
    TitleChanged(Option<String>),
    /// OSC 52 clipboard write.
    ClipboardStore(String),
    /// OSC 7 current-directory notification.
    CurrentDir(PathBuf),
    /// Terminal mode flags transitioned (DECSET/DECRST etc.).
    ModeChanged {
        /// Mode names that transitioned from unset to set.
        added: Vec<String>,
        /// Mode names that transitioned from set to unset.
        removed: Vec<String>,
    },
    /// The PTY child process exited. Carried over the wire so a thin client can
    /// mark the surface dead (the daemon, not the VT, raises this).
    ChildExit {
        /// Exit code; `None` if the wait itself failed.
        code: Option<i32>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vt_event_serde_round_trips_all_variants() {
        let cases = vec![
            VtEvent::Bell,
            VtEvent::TitleChanged(Some("t".into())),
            VtEvent::TitleChanged(None),
            VtEvent::ClipboardStore("c".into()),
            VtEvent::CurrentDir(std::path::PathBuf::from("/tmp")),
            VtEvent::ModeChanged {
                added: vec!["a".into()],
                removed: vec![],
            },
            VtEvent::ChildExit { code: Some(0) },
            VtEvent::ChildExit { code: None },
        ];
        for ev in cases {
            let bytes = rmp_serde::to_vec(&ev).unwrap();
            let back: VtEvent = rmp_serde::from_slice(&bytes).unwrap();
            assert_eq!(ev, back);
        }
    }
}
