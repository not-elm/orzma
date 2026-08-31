//! The orzma APC webview request and its wire parser.
//!
//! `Executor::apc_dispatch` hands raw APC payloads here. What comes
//! back is what the byte stream asked for, before the VT has decided
//! anything: resolving a mount into a placement — and so into a
//! `VtSignal::WebviewMount` or `VtSignal::WebviewMountRejected` — is the
//! dispatcher's job, which is why this module needs no device state.

use crate::placement::PlacementSize;
use std::str;

/// What an orzma APC payload asked for: an inline mount or unmount of a
/// registered view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WebviewApcRequest {
    /// Mount a registered webview INLINE at the cursor anchor, sized in cells.
    Mount {
        /// The registered view's id, addressed later by unmount and eviction.
        view_id: String,
        /// The cell rectangle the mount reserves.
        size: PlacementSize,
        /// Client-assigned instance id (Kitty placement model); `None` is the
        /// implicit default instance. `(view_id, instance_id)` is the address.
        instance_id: Option<String>,
    },
    /// Unmount webview(s): a specific `(view_id, instance_id)`, all
    /// instances of a `view_id`, or all for this terminal.
    ///
    /// # Invariants
    /// `view_id == None` implies `instance_id == None` (an instance is
    /// addressable only alongside its view id; enforced at the capture stage).
    Unmount {
        /// The view to unmount; `None` unmounts every view for this terminal.
        view_id: Option<String>,
        /// The instance to unmount; `None` unmounts every instance of `view_id`.
        instance_id: Option<String>,
    },
}

impl WebviewApcRequest {
    /// Parses an orzma APC payload into the verb it names, or `None`
    /// when the payload is not a well-formed orzma webview verb.
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        if MAX_APC_LEN < bytes.len() {
            return None;
        }
        let body = bytes.strip_prefix(ORZMA_APC_PREFIX)?;
        if body.is_empty() {
            return None;
        }
        let body = str::from_utf8(body).ok()?;
        let mut fields = body.split(';');
        let action_name = fields.next()?;
        let payload = fields.next();
        if fields.next().is_some() {
            return None;
        }
        match action_name {
            "mount" => parse_mount_action(payload?),
            "unmount" => parse_unmount_action(payload),
            _ => None,
        }
    }
}

const MAX_VIEW_ID: usize = 128;
/// Upper bound on a mount's reserved rows. With the ~2:1 terminal cell
/// aspect and DPR 2, a 200-row x 400-col mount is a near-square pixel
/// region staying under the common 8192 px GPU texture dimension limit.
const MAX_ROWS: u16 = 200;
/// Upper bound on a mount's reserved cols; see `MAX_ROWS` for the sizing
/// envelope.
const MAX_COLS: u16 = 400;
const MAX_APC_LEN: usize = 1024;
const ORZMA_APC_PREFIX: &[u8; 1] = b"O";

fn parse_mount_action(payload: &str) -> Option<WebviewApcRequest> {
    let fields = payload.split(',');
    let mut view_id = None;
    let mut rows = None;
    let mut cols = None;
    let mut instance_id = None;
    for f in fields {
        let mut params = f.split('=');
        let k = params.next()?;
        let v = params.next()?;
        if params.next().is_some() {
            return None;
        }
        match k {
            "c" if cols.is_none() => {
                let c = v.parse::<u16>().ok()?;
                if c == 0 || MAX_COLS < c {
                    return None;
                }
                cols.replace(c);
            }
            "r" if rows.is_none() => {
                let r = v.parse().ok()?;
                if r == 0 || MAX_ROWS < r {
                    return None;
                }
                rows.replace(r);
            }
            "v" if view_id.is_none() => {
                let v = v.parse::<String>().ok()?;
                if !valid_view_id(&v) {
                    return None;
                }
                view_id.replace(v);
            }
            "n" if instance_id.is_none() => {
                let v = v.parse::<String>().ok()?;
                if !valid_view_id(&v) {
                    return None;
                }
                instance_id.replace(v);
            }
            _ => return None,
        }
    }
    Some(WebviewApcRequest::Mount {
        size: PlacementSize {
            rows: rows?,
            cols: cols?,
        },
        view_id: view_id?,
        instance_id,
    })
}

fn parse_unmount_action(payload: Option<&str>) -> Option<WebviewApcRequest> {
    let mut view_id = None;
    let mut instance_id = None;
    if let Some(payload) = payload {
        for f in payload.split(',') {
            let mut p = f.split('=');
            let k = p.next()?;
            let v = p.next()?;
            if p.next().is_some() {
                return None;
            }
            match k {
                "v" if view_id.is_none() => {
                    if !valid_view_id(v) {
                        return None;
                    }
                    view_id.replace(v.to_string());
                }
                "n" if instance_id.is_none() & view_id.is_some() => {
                    if !valid_view_id(v) {
                        return None;
                    }
                    instance_id.replace(v.to_string());
                }
                _ => return None,
            }
        }
    }
    Some(WebviewApcRequest::Unmount {
        view_id,
        instance_id,
    })
}

fn valid_view_id(view_id: &str) -> bool {
    if view_id.is_empty() || MAX_VIEW_ID < view_id.len() {
        return false;
    }
    // NOTE: The accepted set must stay the documented view-id charset
    // `[A-Za-z0-9._-]` (docs/orzma_webview_protocol.md): the control
    // plane's mint_id() guarantees its base32 handles (`a-z2-7`) are
    // valid view ids, so narrowing this — e.g. to alphabetic only —
    // rejects nearly every minted handle.
    view_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(payload: &str) -> Option<WebviewApcRequest> {
        WebviewApcRequest::parse(payload.as_bytes())
    }

    #[test]
    fn mount_parses_required_keys() {
        assert_eq!(
            parse("Omount;v=memo,r=3,c=20"),
            Some(WebviewApcRequest::Mount {
                view_id: "memo".into(),
                size: PlacementSize { rows: 3, cols: 20 },
                instance_id: None,
            })
        );
    }

    #[test]
    fn mount_parses_instance_id() {
        assert_eq!(
            parse("Omount;v=memo,r=3,c=20,n=a"),
            Some(WebviewApcRequest::Mount {
                view_id: "memo".into(),
                size: PlacementSize { rows: 3, cols: 20 },
                instance_id: Some("a".into()),
            })
        );
    }

    #[test]
    fn mount_keys_are_order_independent() {
        let expected = Some(WebviewApcRequest::Mount {
            view_id: "memo".into(),
            size: PlacementSize { rows: 3, cols: 20 },
            instance_id: None,
        });
        for payload in ["Omount;c=20,r=3,v=memo", "Omount;r=3,v=memo,c=20"] {
            assert_eq!(parse(payload), expected, "payload={payload}");
        }
    }

    /// Asserts that every character class of the documented view-id
    /// charset `[A-Za-z0-9._-]` is accepted.
    ///
    /// The control plane mints lowercase base32 handles from `a-z2-7`
    /// and guarantees they are valid view ids, so the parser must not
    /// be stricter than the documented charset.
    ///
    /// Case: a program mounts the handle the control socket handed it,
    /// digits and all.
    #[test]
    fn view_id_accepts_the_documented_charset() {
        for id in ["mfrgg2lt2y", "DYN1", "my-view", "a.b_c"] {
            assert_eq!(
                parse(&format!("Omount;v={id},r=3,c=20")),
                Some(WebviewApcRequest::Mount {
                    view_id: id.into(),
                    size: PlacementSize { rows: 3, cols: 20 },
                    instance_id: None,
                }),
                "id={id}"
            );
        }
    }

    #[test]
    fn view_id_length_boundary() {
        let max = "x".repeat(MAX_VIEW_ID);
        assert_eq!(
            parse(&format!("Omount;v={max},r=3,c=20")),
            Some(WebviewApcRequest::Mount {
                view_id: max.clone(),
                size: PlacementSize { rows: 3, cols: 20 },
                instance_id: None,
            }),
            "a view id of exactly MAX_VIEW_ID chars is the accepted maximum"
        );
        let over = "x".repeat(MAX_VIEW_ID + 1);
        assert_eq!(
            parse(&format!("Omount;v={over},r=3,c=20")),
            None,
            "a view id beyond MAX_VIEW_ID chars is rejected"
        );
    }

    #[test]
    fn unmount_without_target_is_unmount_all() {
        assert_eq!(
            parse("Ounmount"),
            Some(WebviewApcRequest::Unmount {
                view_id: None,
                instance_id: None,
            })
        );
    }

    #[test]
    fn unmount_view_only() {
        assert_eq!(
            parse("Ounmount;v=memo"),
            Some(WebviewApcRequest::Unmount {
                view_id: Some("memo".into()),
                instance_id: None,
            })
        );
    }

    #[test]
    fn unmount_view_and_instance() {
        assert_eq!(
            parse("Ounmount;v=memo,n=a"),
            Some(WebviewApcRequest::Unmount {
                view_id: Some("memo".into()),
                instance_id: Some("a".into()),
            })
        );
    }

    #[test]
    fn mount_missing_required_key_rejected() {
        for payload in [
            "Omount",
            "Omount;r=3,c=20",
            "Omount;v=memo,c=20",
            "Omount;v=memo,r=3",
        ] {
            assert_eq!(parse(payload), None, "payload={payload}");
        }
    }

    #[test]
    fn mount_out_of_range_dims_rejected() {
        for payload in [
            "Omount;v=memo,r=0,c=20".to_string(),
            format!("Omount;v=memo,r={},c=20", MAX_ROWS + 1),
            "Omount;v=memo,r=3,c=0".to_string(),
            format!("Omount;v=memo,r=3,c={}", MAX_COLS + 1),
        ] {
            assert_eq!(parse(&payload), None, "payload={payload}");
        }
    }

    #[test]
    fn mount_non_digit_dims_rejected() {
        for payload in ["Omount;v=memo,r=x,c=20", "Omount;v=memo,r=,c=20"] {
            assert_eq!(parse(payload), None, "payload={payload}");
        }
    }

    #[test]
    fn bad_view_id_rejected() {
        for payload in [
            "Omount;v=../etc/passwd,r=3,c=20",
            "Omount;v=,r=3,c=20",
            "Omount;v=me mo,r=3,c=20",
            "Omount;v=me=mo,r=3,c=20",
        ] {
            assert_eq!(parse(payload), None, "payload={payload}");
        }
    }

    #[test]
    fn mount_bad_instance_id_rejected() {
        for payload in [
            "Omount;v=memo,r=3,c=20,n=",
            "Omount;v=memo,r=3,c=20,n=../etc",
        ] {
            assert_eq!(parse(payload), None, "payload={payload}");
        }
    }

    #[test]
    fn empty_params_section_rejected() {
        for payload in ["Omount;", "Ounmount;"] {
            assert_eq!(
                parse(payload),
                None,
                "an absent params section is a valid omission but an empty one is malformed; payload={payload}"
            );
        }
    }

    #[test]
    fn extra_section_rejected() {
        for payload in ["Omount;v=memo,r=3,c=20;x", "Ounmount;v=memo;x"] {
            assert_eq!(
                parse(payload),
                None,
                "a third ';' section is reserved and rejected; payload={payload}"
            );
        }
    }

    #[test]
    fn unmount_empty_view_value_rejected() {
        assert_eq!(
            parse("Ounmount;v="),
            None,
            "an absent v key means unmount-all; an empty value is malformed"
        );
    }

    #[test]
    fn unmount_instance_without_view_rejected() {
        assert_eq!(
            parse("Ounmount;n=a"),
            None,
            "an instance id is addressable only alongside a view id"
        );
    }

    #[test]
    fn unmount_bad_instance_rejected() {
        assert_eq!(parse("Ounmount;v=memo,n=../x"), None);
    }

    #[test]
    fn unmount_with_mount_only_keys_rejected() {
        for payload in ["Ounmount;v=memo,r=3", "Ounmount;c=20"] {
            assert_eq!(parse(payload), None, "payload={payload}");
        }
    }

    #[test]
    fn unknown_verb_rejected() {
        for payload in [
            "Oresize;v=memo",
            "OMOUNT;v=memo,r=3,c=20",
            "Om;v=memo,r=3,c=20",
        ] {
            assert_eq!(parse(payload), None, "payload={payload}");
        }
    }

    #[test]
    fn foreign_or_missing_prefix_rejected() {
        for payload in ["Ga=T,f=100", "mount;v=memo,r=3,c=20", "", "O"] {
            assert_eq!(parse(payload), None, "payload={payload:?}");
        }
    }

    #[test]
    fn unknown_key_rejected() {
        assert_eq!(parse("Omount;v=memo,r=3,c=20,z=9"), None);
    }

    #[test]
    fn duplicate_key_rejected() {
        for payload in ["Omount;v=memo,v=memo,r=3,c=20", "Omount;v=a,r=3,r=4,c=20"] {
            assert_eq!(parse(payload), None, "payload={payload}");
        }
    }

    #[test]
    fn malformed_pair_rejected() {
        for payload in [
            "Omount;vmemo,r=3,c=20",
            "Ounmount;v=memo,",
            "Omount;,v=memo,r=3,c=20",
            "Ounmount;v=memo,,n=a",
        ] {
            assert_eq!(parse(payload), None, "payload={payload}");
        }
    }

    #[test]
    fn out_of_charset_bytes_rejected() {
        assert_eq!(
            WebviewApcRequest::parse(b"Omount;v=me\x07mo,r=3,c=20"),
            None
        );
        assert_eq!(
            WebviewApcRequest::parse(b"Omount;v=me\x1bmo,r=3,c=20"),
            None
        );
        assert_eq!(
            WebviewApcRequest::parse("Omount;v=めも,r=3,c=20".as_bytes()),
            None,
            "multi-byte UTF-8 is outside the APC command-string charset"
        );
    }

    #[test]
    fn oversized_payload_rejected() {
        let mut huge = b"O".to_vec();
        huge.resize(MAX_APC_LEN + 1, b'a');
        assert_eq!(
            WebviewApcRequest::parse(&huge),
            None,
            "payloads beyond MAX_APC_LEN are rejected before field parsing"
        );
    }
}
