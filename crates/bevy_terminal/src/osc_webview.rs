//! Sibling `vte::Perform` that captures the ozmux private OSC for webview
//! mount/unmount and forwards it as a `ControlFrame::OscWebview` — but only when
//! the shared default-off gate is enabled.

use crate::vt::listener::{ControlFrame, OscWebviewVerb};
use crossbeam_channel::Sender;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use vte::Perform;

/// The ozmux private OSC code for webview control. Outside vte 0.15 /
/// alacritty 0.26's dispatched set, so the main parser drops it harmlessly.
pub(crate) const OSC_WEBVIEW_CODE: &[u8] = b"5379";

const MAX_VIEW_ID: usize = 128;

/// A `vte::Perform` that captures OSC 5379 payloads and emits
/// `ControlFrame::OscWebview` on the control channel when the gate is on.
pub(crate) struct OscWebviewCapture {
    control_tx: Sender<ControlFrame>,
    gate: Arc<AtomicBool>,
}

impl OscWebviewCapture {
    /// Builds a capture that sends frames on `control_tx` only when `gate` is `true`.
    pub(crate) fn new(control_tx: Sender<ControlFrame>, gate: Arc<AtomicBool>) -> Self {
        Self { control_tx, gate }
    }
}

fn valid_view_id(s: &[u8]) -> Option<String> {
    if s.is_empty() || s.len() > MAX_VIEW_ID {
        return None;
    }
    if !s
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(String::from_utf8_lossy(s).into_owned())
}

impl Perform for OscWebviewCapture {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.first().copied() != Some(OSC_WEBVIEW_CODE) {
            return;
        }
        if !self.gate.load(Ordering::Relaxed) {
            return;
        }
        let verb = match params.get(1).copied() {
            Some(b"mount") => match params.get(2).copied().and_then(valid_view_id) {
                Some(view_id) => OscWebviewVerb::Mount { view_id },
                None => return,
            },
            Some(b"unmount") => {
                // NOTE: a present-but-invalid view-id is a malformed sequence, not
                // an implicit "unmount any". Drop it; only an ABSENT third param
                // means "unmount the pane's OSC webview".
                let view_id = match params.get(2).copied() {
                    Some(raw) => match valid_view_id(raw) {
                        Some(v) => Some(v),
                        None => return,
                    },
                    None => None,
                };
                OscWebviewVerb::Unmount { view_id }
            }
            _ => return,
        };
        if let Err(e) = self
            .control_tx
            .send(ControlFrame::OscWebview { verb, anchor: None })
        {
            tracing::warn!(?e, "control_tx send(OscWebview) failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    #[test]
    fn gate_off_drops_sequence() {
        let (tx, rx) = unbounded();
        let mut cap = OscWebviewCapture::new(tx, Arc::new(AtomicBool::new(false)));
        cap.osc_dispatch(&[OSC_WEBVIEW_CODE, b"mount", b"dash"], true);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn gate_on_emits_mount() {
        let (tx, rx) = unbounded();
        let mut cap = OscWebviewCapture::new(tx, Arc::new(AtomicBool::new(true)));
        cap.osc_dispatch(&[OSC_WEBVIEW_CODE, b"mount", b"dash"], true);
        assert_eq!(
            rx.try_recv(),
            Ok(ControlFrame::OscWebview {
                verb: OscWebviewVerb::Mount {
                    view_id: "dash".into()
                },
                anchor: None,
            })
        );
    }

    #[test]
    fn other_osc_code_ignored() {
        let (tx, rx) = unbounded();
        let mut cap = OscWebviewCapture::new(tx, Arc::new(AtomicBool::new(true)));
        cap.osc_dispatch(&[b"7", b"file://localhost/tmp"], true);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn bad_view_id_rejected() {
        let (tx, rx) = unbounded();
        let mut cap = OscWebviewCapture::new(tx, Arc::new(AtomicBool::new(true)));
        cap.osc_dispatch(&[OSC_WEBVIEW_CODE, b"mount", b"../etc/passwd"], true);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn unmount_without_view_id_targets_any() {
        let (tx, rx) = unbounded();
        let mut cap = OscWebviewCapture::new(tx, Arc::new(AtomicBool::new(true)));
        cap.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount"], true);
        assert_eq!(
            rx.try_recv(),
            Ok(ControlFrame::OscWebview {
                verb: OscWebviewVerb::Unmount { view_id: None },
                anchor: None,
            })
        );
    }

    #[test]
    fn unmount_with_valid_view_id_targets_it() {
        let (tx, rx) = unbounded();
        let mut cap = OscWebviewCapture::new(tx, Arc::new(AtomicBool::new(true)));
        cap.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount", b"dash"], true);
        assert_eq!(
            rx.try_recv(),
            Ok(ControlFrame::OscWebview {
                verb: OscWebviewVerb::Unmount {
                    view_id: Some("dash".into())
                },
                anchor: None,
            })
        );
    }

    #[test]
    fn unmount_with_invalid_view_id_dropped() {
        let (tx, rx) = unbounded();
        let mut cap = OscWebviewCapture::new(tx, Arc::new(AtomicBool::new(true)));
        cap.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount", b"../etc/passwd"], true);
        assert!(
            rx.try_recv().is_err(),
            "a present-but-invalid unmount view-id must drop, not unmount-any"
        );
    }
}
