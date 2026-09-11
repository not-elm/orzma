//! The orzma APC webview request and its wire parser.

use crate::placement::{InstanceId, MAX_COLS, MAX_ROWS, PlacementSize};
use std::str;

/// What an orzma APC payload asked for: an inline mount or unmount of a
/// registered webview instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WebviewApcRequest {
    /// Mount a registered webview INLINE at the cursor anchor, sized in cells.
    Mount {
        /// The host-minted instance this mount registers.
        instance: InstanceId,
        /// The cell rectangle the mount reserves.
        size: PlacementSize,
    },
    /// Unmount one instance, or — with no params section — every
    /// placement on this terminal.
    Unmount {
        /// The instance to unmount; `None` unmounts every placement.
        instance: Option<InstanceId>,
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

const MAX_APC_LEN: usize = 1024;
const ORZMA_APC_PREFIX: &[u8; 1] = b"O";

fn parse_mount_action(payload: &str) -> Option<WebviewApcRequest> {
    let fields = payload.split(',');
    let mut instance = None;
    let mut rows = None;
    let mut cols = None;
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
            "n" if instance.is_none() => {
                instance.replace(v.parse::<InstanceId>().ok()?);
            }
            _ => return None,
        }
    }
    Some(WebviewApcRequest::Mount {
        instance: instance?,
        size: PlacementSize {
            rows: rows?,
            cols: cols?,
        },
    })
}

fn parse_unmount_action(payload: Option<&str>) -> Option<WebviewApcRequest> {
    let Some(payload) = payload else {
        return Some(WebviewApcRequest::Unmount { instance: None });
    };
    let mut instance = None;
    for f in payload.split(',') {
        let mut p = f.split('=');
        let k = p.next()?;
        let v = p.next()?;
        if p.next().is_some() {
            return None;
        }
        match k {
            "n" if instance.is_none() => {
                instance.replace(v.parse::<InstanceId>().ok()?);
            }
            _ => return None,
        }
    }
    Some(WebviewApcRequest::Unmount {
        instance: Some(instance?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "3f5a9c02d1e84b7690ab3cde12f45678";

    fn parse(payload: &str) -> Option<WebviewApcRequest> {
        WebviewApcRequest::parse(payload.as_bytes())
    }

    /// Asserts that a mount parses from the `n` / `r` / `c` trio in any
    /// order and rejects a payload that omits any of them.
    ///
    /// Case: a program reserves a 24x80 rectangle for an instance the
    /// control plane just handed it.
    #[test]
    fn a_mount_parses_its_three_required_keys_in_any_order() {
        let expected = WebviewApcRequest::Mount {
            instance: ID.parse().expect("the fixture is a valid id"),
            size: PlacementSize { rows: 24, cols: 80 },
        };
        assert_eq!(
            parse(&format!("Omount;n={ID},r=24,c=80")),
            Some(expected.clone())
        );
        assert_eq!(parse(&format!("Omount;c=80,n={ID},r=24")), Some(expected));
        assert_eq!(parse("Omount;r=24,c=80"), None);
        assert_eq!(parse(&format!("Omount;n={ID},c=80")), None);
        assert_eq!(parse(&format!("Omount;n={ID},r=24")), None);
    }

    /// Asserts that a mount naming the retired `v=` key is malformed
    /// rather than silently ignoring the key.
    ///
    /// Case: a program built against the previous protocol writes the
    /// handle-addressed form.
    #[test]
    fn a_mount_naming_the_retired_view_key_is_malformed() {
        assert_eq!(parse(&format!("Omount;v=abc,n={ID},r=24,c=80")), None);
        assert_eq!(parse("Omount;v=abc,r=24,c=80"), None);
    }

    /// Asserts that a mount whose instance is not exactly 32 lowercase
    /// hex digits is malformed, and that a repeated key is too.
    ///
    /// Case: a program fabricates an instance id instead of using the one
    /// the control plane minted.
    #[test]
    fn a_mount_with_a_malformed_instance_or_a_repeated_key_is_rejected() {
        assert_eq!(parse("Omount;n=abc,r=24,c=80"), None);
        assert_eq!(
            parse(&format!("Omount;n={},r=24,c=80", ID.to_uppercase())),
            None
        );
        assert_eq!(parse(&format!("Omount;n={ID},n={ID},r=24,c=80")), None);
    }

    /// Asserts that an unmount takes the instance key alone, in any
    /// position, and that an empty params section stays malformed.
    ///
    /// Case: a program tears one placement down and later asks the
    /// terminal to drop every placement it still holds.
    #[test]
    fn an_unmount_addresses_one_instance_or_all() {
        let one = WebviewApcRequest::Unmount {
            instance: Some(ID.parse().expect("the fixture is a valid id")),
        };
        assert_eq!(parse(&format!("Ounmount;n={ID}")), Some(one));
        assert_eq!(
            parse("Ounmount"),
            Some(WebviewApcRequest::Unmount { instance: None })
        );
        assert_eq!(parse("Ounmount;"), None);
        assert_eq!(parse("Ounmount;n="), None);
        assert_eq!(parse(&format!("Ounmount;v=abc,n={ID}")), None);
    }

    #[test]
    fn mount_out_of_range_dims_rejected() {
        for payload in [
            format!("Omount;n={ID},r=0,c=20"),
            format!("Omount;n={ID},r={},c=20", MAX_ROWS + 1),
            format!("Omount;n={ID},r=3,c=0"),
            format!("Omount;n={ID},r=3,c={}", MAX_COLS + 1),
        ] {
            assert_eq!(parse(&payload), None, "payload={payload}");
        }
    }

    #[test]
    fn mount_non_digit_dims_rejected() {
        for payload in [
            format!("Omount;n={ID},r=x,c=20"),
            format!("Omount;n={ID},r=,c=20"),
        ] {
            assert_eq!(parse(&payload), None, "payload={payload}");
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
        for payload in [
            format!("Omount;n={ID},r=3,c=20;x"),
            format!("Ounmount;n={ID};x"),
        ] {
            assert_eq!(
                parse(&payload),
                None,
                "a third ';' section is reserved and rejected; payload={payload}"
            );
        }
    }

    #[test]
    fn unknown_verb_rejected() {
        for payload in [
            format!("Oresize;n={ID}"),
            format!("OMOUNT;n={ID},r=3,c=20"),
            format!("Om;n={ID},r=3,c=20"),
        ] {
            assert_eq!(parse(&payload), None, "payload={payload}");
        }
    }

    #[test]
    fn foreign_or_missing_prefix_rejected() {
        for payload in [
            "Ga=T,f=100".to_string(),
            format!("mount;n={ID},r=3,c=20"),
            String::new(),
            "O".to_string(),
        ] {
            assert_eq!(parse(&payload), None, "payload={payload:?}");
        }
    }

    #[test]
    fn unknown_key_rejected() {
        assert_eq!(parse(&format!("Omount;n={ID},r=3,c=20,z=9")), None);
    }

    #[test]
    fn duplicate_key_rejected() {
        for payload in [
            format!("Omount;n={ID},n={ID},r=3,c=20"),
            format!("Omount;n={ID},r=3,r=4,c=20"),
        ] {
            assert_eq!(parse(&payload), None, "payload={payload}");
        }
    }

    #[test]
    fn malformed_pair_rejected() {
        for payload in [
            "Omount;vmemo,r=3,c=20".to_string(),
            format!("Ounmount;n={ID},"),
            format!("Omount;,n={ID},r=3,c=20"),
            format!("Ounmount;n={ID},,n=a"),
        ] {
            assert_eq!(parse(&payload), None, "payload={payload}");
        }
    }

    #[test]
    fn out_of_charset_bytes_rejected() {
        assert_eq!(
            WebviewApcRequest::parse(b"Omount;n=me\x07mo,r=3,c=20"),
            None
        );
        assert_eq!(
            WebviewApcRequest::parse(b"Omount;n=me\x1bmo,r=3,c=20"),
            None
        );
        assert_eq!(
            WebviewApcRequest::parse("Omount;n=めも,r=3,c=20".as_bytes()),
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
