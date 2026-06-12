//! Sibling `vte::Perform` that captures OSC 5379 payloads and buffers one
//! parsed verb per sequence for `TerminalHandle::advance` to drain at the
//! `advance_until_terminated` stop point — where the inline anchor is stamped.
//! Never sends frames itself.

use crate::vt::listener::OscWebviewVerb;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use vte::Perform;

/// The ozmux private OSC code for webview control. Outside vte 0.15 /
/// alacritty 0.26's dispatched set, so the main parser drops it harmlessly.
pub(crate) const OSC_WEBVIEW_CODE: &[u8] = b"5379";

const MAX_VIEW_ID: usize = 128;
const MAX_ROWS: u16 = 200;
const MAX_COLS: u16 = 400;

/// A `vte::Perform` that parses OSC 5379 payloads and buffers ONE verb for
/// `TerminalHandle::advance` to drain at the `advance_until_terminated`
/// stop point (where the anchor is stamped). It never sends frames itself.
pub(crate) struct OscWebviewCapture {
    gate: Arc<AtomicBool>,
    pending: Option<OscWebviewVerb>,
}

impl OscWebviewCapture {
    /// Builds a capture that buffers verbs only while `gate` is `true`.
    pub(crate) fn new(gate: Arc<AtomicBool>) -> Self {
        Self {
            gate,
            pending: None,
        }
    }

    /// Takes the buffered verb, clearing `terminated()`.
    // NOTE: the caller MUST take the pending verb after every
    // advance_until_terminated stop; a stuck pending makes the next call
    // return 0 forever (infinite loop in TerminalHandle::advance).
    // NOTE: cfg_attr(not(test)) because the tests below call this method —
    // a bare #[expect] would be unfulfilled in test builds and warn.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "drain point for the follow-up advance-loop task")
    )]
    pub(crate) fn take_pending(&mut self) -> Option<OscWebviewVerb> {
        self.pending.take()
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

fn parse_dim(raw: Option<&[u8]>, max: u16) -> Option<u16> {
    let s = std::str::from_utf8(raw?).ok()?;
    let v: u16 = s.parse().ok()?;
    (1..=max).contains(&v).then_some(v)
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
            Some(b"mount-inline") => {
                let Some(view_id) = params.get(2).copied().and_then(valid_view_id) else {
                    return;
                };
                let Some(rows) = parse_dim(params.get(3).copied(), MAX_ROWS) else {
                    return;
                };
                let Some(cols) = parse_dim(params.get(4).copied(), MAX_COLS) else {
                    return;
                };
                OscWebviewVerb::MountInline {
                    view_id,
                    rows,
                    cols,
                }
            }
            Some(b"unmount-inline") => {
                let view_id = match params.get(2).copied() {
                    Some(raw) => match valid_view_id(raw) {
                        Some(v) => Some(v),
                        None => return,
                    },
                    None => None,
                };
                OscWebviewVerb::UnmountInline { view_id }
            }
            _ => return,
        };
        self.pending = Some(verb);
    }

    fn terminated(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(gate_on: bool) -> OscWebviewCapture {
        OscWebviewCapture::new(Arc::new(AtomicBool::new(gate_on)))
    }

    #[test]
    fn gate_off_drops_sequence() {
        let mut c = cap(false);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"mount", b"dash"], true);
        assert!(c.take_pending().is_none());
    }

    #[test]
    fn gate_on_buffers_mount_and_terminated_reflects_pending() {
        let mut c = cap(true);
        assert!(!Perform::terminated(&c));
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"mount", b"dash"], true);
        assert!(Perform::terminated(&c), "pending verb must set terminated");
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Mount {
                view_id: "dash".into()
            })
        );
        assert!(
            !Perform::terminated(&c),
            "take_pending must clear terminated or advance_until_terminated loops forever"
        );
    }

    #[test]
    fn other_osc_code_ignored() {
        let mut c = cap(true);
        c.osc_dispatch(&[b"7", b"file://localhost/tmp"], true);
        assert!(c.take_pending().is_none());
    }

    #[test]
    fn bad_view_id_rejected() {
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"mount", b"../etc/passwd"], true);
        assert!(c.take_pending().is_none());
    }

    #[test]
    fn unmount_without_view_id_targets_any() {
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount"], true);
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Unmount { view_id: None })
        );
    }

    #[test]
    fn unmount_with_invalid_view_id_dropped() {
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount", b"../etc/passwd"], true);
        assert!(c.take_pending().is_none());
    }

    #[test]
    fn mount_inline_parses_rows_cols() {
        let mut c = cap(true);
        c.osc_dispatch(
            &[OSC_WEBVIEW_CODE, b"mount-inline", b"memo", b"3", b"20"],
            true,
        );
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::MountInline {
                view_id: "memo".into(),
                rows: 3,
                cols: 20,
            })
        );
    }

    #[test]
    fn mount_inline_out_of_range_dims_dropped() {
        for (r, w) in [
            ("0", "20"),
            ("201", "20"),
            ("3", "0"),
            ("3", "401"),
            ("x", "20"),
        ] {
            let mut c = cap(true);
            c.osc_dispatch(
                &[
                    OSC_WEBVIEW_CODE,
                    b"mount-inline",
                    b"memo",
                    r.as_bytes(),
                    w.as_bytes(),
                ],
                true,
            );
            assert!(
                c.take_pending().is_none(),
                "rows={r} cols={w} must be malformed"
            );
        }
    }

    #[test]
    fn mount_inline_missing_dims_dropped() {
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"mount-inline", b"memo"], true);
        assert!(c.take_pending().is_none());
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"mount-inline", b"memo", b"3"], true);
        assert!(c.take_pending().is_none());
    }

    #[test]
    fn unmount_inline_absent_param_is_all_but_empty_param_is_malformed() {
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount-inline"], true);
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::UnmountInline { view_id: None })
        );
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount-inline", b""], true);
        assert!(
            c.take_pending().is_none(),
            "empty third param is malformed, not unmount-all"
        );
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount-inline", b"memo"], true);
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::UnmountInline {
                view_id: Some("memo".into())
            })
        );
    }
}
