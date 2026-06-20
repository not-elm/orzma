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
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
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
            Some(b"mount") => {
                let Some(view_id) = params.get(2).copied().and_then(valid_view_id) else {
                    return;
                };
                let Some(rows) = parse_dim(params.get(3).copied(), MAX_ROWS) else {
                    return;
                };
                let Some(cols) = parse_dim(params.get(4).copied(), MAX_COLS) else {
                    return;
                };
                let instance_id = match params.get(5).copied() {
                    Some(raw) => match valid_view_id(raw) {
                        Some(v) => Some(v),
                        None => return,
                    },
                    None => None,
                };
                OscWebviewVerb::Mount {
                    view_id,
                    rows,
                    cols,
                    instance_id,
                }
            }
            Some(b"unmount") => {
                // NOTE: a present-but-invalid view id is malformed, not "unmount
                // any"; only an ABSENT third param means "all inline on this
                // terminal". An empty third param (`unmount ; ;`) is
                // rejected by valid_view_id.
                let view_id = match params.get(2).copied() {
                    Some(raw) => match valid_view_id(raw) {
                        Some(v) => Some(v),
                        None => return,
                    },
                    None => None,
                };
                // NOTE: an instance id is addressable only alongside a view id
                // (Kitty placement model); the empty-view-id case is already
                // dropped above, so reaching here with `view_id == None` and a
                // present fourth param is impossible, but the guard keeps the
                // `view_id == None ⟹ instance_id == None` invariant explicit.
                let instance_id = match params.get(3).copied() {
                    Some(_) if view_id.is_none() => return,
                    Some(raw) => match valid_view_id(raw) {
                        Some(v) => Some(v),
                        None => return,
                    },
                    None => None,
                };
                OscWebviewVerb::Unmount {
                    view_id,
                    instance_id,
                }
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
        c.osc_dispatch(
            &[OSC_WEBVIEW_CODE, b"mount", b"memo", b"3", b"20"],
            true,
        );
        assert!(c.take_pending().is_none());
    }

    #[test]
    fn gate_on_buffers_verb_and_terminated_reflects_pending() {
        let mut c = cap(true);
        assert!(!Perform::terminated(&c));
        c.osc_dispatch(
            &[OSC_WEBVIEW_CODE, b"mount", b"memo", b"3", b"20"],
            true,
        );
        assert!(Perform::terminated(&c), "pending verb must set terminated");
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Mount {
                view_id: "memo".into(),
                rows: 3,
                cols: 20,
                instance_id: None,
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
        c.osc_dispatch(
            &[
                OSC_WEBVIEW_CODE,
                b"mount",
                b"../etc/passwd",
                b"3",
                b"20",
            ],
            true,
        );
        assert!(c.take_pending().is_none());
    }

    #[test]
    fn mount_parses_rows_cols() {
        let mut c = cap(true);
        c.osc_dispatch(
            &[OSC_WEBVIEW_CODE, b"mount", b"memo", b"3", b"20"],
            true,
        );
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Mount {
                view_id: "memo".into(),
                rows: 3,
                cols: 20,
                instance_id: None,
            })
        );
    }

    #[test]
    fn mount_out_of_range_dims_dropped() {
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
                    b"mount",
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
    fn mount_minimum_dims_accepted() {
        let mut c = cap(true);
        c.osc_dispatch(
            &[OSC_WEBVIEW_CODE, b"mount", b"memo", b"1", b"1"],
            true,
        );
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Mount {
                view_id: "memo".into(),
                rows: 1,
                cols: 1,
                instance_id: None,
            })
        );
    }

    #[test]
    fn mount_maximum_dims_accepted() {
        let mut c = cap(true);
        c.osc_dispatch(
            &[OSC_WEBVIEW_CODE, b"mount", b"memo", b"200", b"400"],
            true,
        );
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Mount {
                view_id: "memo".into(),
                rows: 200,
                cols: 400,
                instance_id: None,
            })
        );
    }

    #[test]
    fn mount_non_digit_dims_dropped() {
        for (r, w) in [("3", "y"), ("+3", "20"), ("3", "+20")] {
            let mut c = cap(true);
            c.osc_dispatch(
                &[
                    OSC_WEBVIEW_CODE,
                    b"mount",
                    b"memo",
                    r.as_bytes(),
                    w.as_bytes(),
                ],
                true,
            );
            assert!(
                c.take_pending().is_none(),
                "rows={r} cols={w} must be malformed (digits only)"
            );
        }
    }

    #[test]
    fn mount_missing_dims_dropped() {
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"mount", b"memo"], true);
        assert!(c.take_pending().is_none());
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"mount", b"memo", b"3"], true);
        assert!(c.take_pending().is_none());
    }

    #[test]
    fn mount_parses_instance_id() {
        let mut c = cap(true);
        c.osc_dispatch(
            &[
                OSC_WEBVIEW_CODE,
                b"mount",
                b"memo",
                b"3",
                b"20",
                b"a",
            ],
            true,
        );
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Mount {
                view_id: "memo".into(),
                rows: 3,
                cols: 20,
                instance_id: Some("a".into()),
            })
        );
    }

    #[test]
    fn mount_absent_instance_id_is_none() {
        let mut c = cap(true);
        c.osc_dispatch(
            &[OSC_WEBVIEW_CODE, b"mount", b"memo", b"3", b"20"],
            true,
        );
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Mount {
                view_id: "memo".into(),
                rows: 3,
                cols: 20,
                instance_id: None,
            })
        );
    }

    #[test]
    fn mount_trailing_empty_instance_id_dropped() {
        let mut c = cap(true);
        c.osc_dispatch(
            &[OSC_WEBVIEW_CODE, b"mount", b"memo", b"3", b"20", b""],
            true,
        );
        assert!(
            c.take_pending().is_none(),
            "a trailing empty instance id (mount;memo;3;20;) is malformed"
        );
    }

    #[test]
    fn mount_bad_instance_id_dropped() {
        let mut c = cap(true);
        c.osc_dispatch(
            &[
                OSC_WEBVIEW_CODE,
                b"mount",
                b"memo",
                b"3",
                b"20",
                b"../etc",
            ],
            true,
        );
        assert!(
            c.take_pending().is_none(),
            "an out-of-charset instance id is malformed"
        );
    }

    #[test]
    fn unmount_absent_param_is_all_but_empty_param_is_malformed() {
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount"], true);
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Unmount {
                view_id: None,
                instance_id: None,
            })
        );
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount", b""], true);
        assert!(
            c.take_pending().is_none(),
            "empty third param is malformed, not unmount-all"
        );
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount", b"memo"], true);
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Unmount {
                view_id: Some("memo".into()),
                instance_id: None,
            })
        );
    }

    #[test]
    fn unmount_parses_view_and_instance() {
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount", b"memo", b"a"], true);
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Unmount {
                view_id: Some("memo".into()),
                instance_id: Some("a".into()),
            })
        );
    }

    #[test]
    fn unmount_view_only_has_no_instance() {
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount", b"memo"], true);
        assert_eq!(
            c.take_pending(),
            Some(OscWebviewVerb::Unmount {
                view_id: Some("memo".into()),
                instance_id: None,
            })
        );
    }

    #[test]
    fn unmount_trailing_empty_instance_dropped() {
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount", b"memo", b""], true);
        assert!(
            c.take_pending().is_none(),
            "unmount;memo; (empty instance) is malformed"
        );
    }

    #[test]
    fn unmount_empty_view_with_instance_dropped() {
        let mut c = cap(true);
        c.osc_dispatch(&[OSC_WEBVIEW_CODE, b"unmount", b"", b"a"], true);
        assert!(
            c.take_pending().is_none(),
            "unmount;;a (empty view id + instance) is malformed"
        );
    }

    #[test]
    fn unmount_bad_instance_dropped() {
        let mut c = cap(true);
        c.osc_dispatch(
            &[OSC_WEBVIEW_CODE, b"unmount", b"memo", b"../x"],
            true,
        );
        assert!(
            c.take_pending().is_none(),
            "an out-of-charset instance id is malformed"
        );
    }
}
