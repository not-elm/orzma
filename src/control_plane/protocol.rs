//! NDJSON wire types for the control plane, internally tagged on `op`. The
//! listener parses one `ClientMsg` per line and replies with one `ServerMsg`
//! per request line. Unknown `op` values fail to parse (strict, matching the
//! OSC parser ethos).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One inbound control-plane request line.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum ClientMsg {
    /// Connection handshake: binds this connection to the pane whose env
    /// carries `token`. Sent once, before any `register`.
    Hello {
        /// The per-surface `$OZMA_TOKEN` value.
        token: String,
    },
    /// Registers a dynamic view and requests a handle.
    Register(RegisterKind),
    /// Releases a previously-registered handle owned by this connection.
    Unregister {
        /// The handle returned by a prior `register`.
        handle: String,
    },
    /// A program's reply to an ozmux-initiated `call` (back-channel).
    Reply {
        /// The global reqId ozmux assigned to the originating `call`.
        #[serde(rename = "reqId")]
        req_id: String,
        /// Whether the call succeeded.
        ok: bool,
        /// The success value (absent ⇒ `null`).
        #[serde(default)]
        value: Value,
        /// The error message when `ok` is false.
        #[serde(default)]
        error: Option<String>,
    },
    /// A program-initiated push event to its handle's mounted webviews.
    Emit {
        /// The handle whose mounted webviews receive the event.
        handle: String,
        /// The event name dispatched to page `window.ozma.on(name, …)`.
        event: String,
        /// The event payload.
        #[serde(default)]
        payload: Value,
    },
    /// Sets (or clears, with `handle: None`) the app-owned focus target for
    /// this connection's surface.
    Focus {
        /// The handle to focus, or `None` to blur.
        #[serde(default)]
        handle: Option<String>,
        /// The mount instance id, or `None` for the default instance.
        #[serde(default)]
        instance: Option<String>,
    },
    /// Navigate a handle's mounted webview in place.
    Navigate {
        /// The target handle.
        handle: String,
        /// What to do.
        action: NavAction,
    },
}

/// A navigation action on an already-registered handle's mounted webview.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NavAction {
    /// Go back in the webview's native session history.
    Back,
    /// Go forward in the webview's native session history.
    Forward,
    /// Reload the current page.
    Reload,
    /// Navigate the existing webview to a new URL.
    To(String),
}

/// A forward-key chord as received on the register wire (host side).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct HostKeyChord {
    /// Modifier names: any of `alt`, `ctrl`, `shift`, `meta`.
    pub(crate) mods: Vec<String>,
    /// The base key: a lowercase char (`h`, `5`), or `tab`/`backtab`/`f1`..`f12`.
    pub(crate) key: String,
}

/// The content source a `register` declares.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RegisterKind {
    /// Serve files under `root` (absolute) at entry `entry`.
    Dir {
        /// Absolute asset root directory.
        root: String,
        /// HTML entry path relative to `root` (e.g. `index.html`).
        entry: String,
        /// Whether the mounted webview accepts pointer/keyboard input.
        #[serde(default = "default_true")]
        interactive: bool,
        /// Chords the host passes through to PTY instead of consuming in CEF.
        #[serde(default)]
        forward_keys: Vec<HostKeyChord>,
        /// User-supplied scripts injected before the page's own scripts run.
        #[serde(default)]
        preload: Vec<String>,
    },
    /// Serve a single dynamic HTML document supplied inline.
    Inline {
        /// The full HTML document.
        html: String,
        /// Whether the mounted webview accepts pointer/keyboard input.
        #[serde(default = "default_true")]
        interactive: bool,
        /// Chords the host passes through to PTY instead of consuming in CEF.
        #[serde(default)]
        forward_keys: Vec<HostKeyChord>,
        /// User-supplied scripts injected before the page's own scripts run.
        #[serde(default)]
        preload: Vec<String>,
    },
    /// Load a remote `http(s)` URL as the top-level document.
    Url {
        /// The `http(s)` URL to load.
        url: String,
        /// Whether the mounted webview accepts pointer/keyboard input.
        #[serde(default = "default_true")]
        interactive: bool,
        /// Whether the `window.ozma` back-channel is injected (opt-in).
        #[serde(default)]
        bridge: bool,
        /// Chords the host passes through to PTY instead of consuming in CEF.
        #[serde(default)]
        forward_keys: Vec<HostKeyChord>,
        /// User-supplied scripts injected before the page's own scripts run.
        #[serde(default)]
        preload: Vec<String>,
    },
}

/// One outbound control-plane reply line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum ServerMsg {
    /// A successful `register` carrying the minted handle.
    Ok {
        /// Always `true`.
        ok: bool,
        /// The opaque handle to mount via `OSC mount;<handle>`.
        handle: String,
    },
    /// A rejected request.
    Err {
        /// Always `false`.
        ok: bool,
        /// A short machine-readable error code.
        error: String,
    },
}

impl ServerMsg {
    /// An `ok` reply carrying the minted handle.
    pub(crate) fn ok(handle: impl Into<String>) -> Self {
        Self::Ok {
            ok: true,
            handle: handle.into(),
        }
    }

    /// An error reply carrying a short code.
    pub(crate) fn err(error: impl Into<String>) -> Self {
        Self::Err {
            ok: false,
            error: error.into(),
        }
    }
}

/// An outbound push notification sent from the control plane to a registered
/// program over the control socket without being a reply to a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum PushMsg {
    /// Fired when the webview first composites (`active: true`) or is
    /// unmounted after compositing (`active: false`).
    Compositing {
        /// The registered handle whose compositing state changed.
        handle: String,
        /// `true` when compositing starts; `false` when it stops.
        active: bool,
    },
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hello() {
        let m: ClientMsg = serde_json::from_str(r#"{"op":"hello","token":"t1"}"#).unwrap();
        assert_eq!(m, ClientMsg::Hello { token: "t1".into() });
    }

    #[test]
    fn parses_dir_register_with_default_interactive() {
        let m: ClientMsg = serde_json::from_str(
            r#"{"op":"register","kind":"dir","root":"/abs","entry":"index.html"}"#,
        )
        .unwrap();
        assert_eq!(
            m,
            ClientMsg::Register(RegisterKind::Dir {
                root: "/abs".into(),
                entry: "index.html".into(),
                interactive: true,
                forward_keys: vec![],
                preload: vec![],
            })
        );
    }

    #[test]
    fn parses_inline_register() {
        let m: ClientMsg = serde_json::from_str(
            r#"{"op":"register","kind":"inline","html":"<h1>x</h1>","interactive":false}"#,
        )
        .unwrap();
        assert_eq!(
            m,
            ClientMsg::Register(RegisterKind::Inline {
                html: "<h1>x</h1>".into(),
                interactive: false,
                forward_keys: vec![],
                preload: vec![],
            })
        );
    }

    #[test]
    fn parses_unregister() {
        let m: ClientMsg = serde_json::from_str(r#"{"op":"unregister","handle":"h1"}"#).unwrap();
        assert_eq!(
            m,
            ClientMsg::Unregister {
                handle: "h1".into()
            }
        );
    }

    #[test]
    fn rejects_unknown_op() {
        assert!(serde_json::from_str::<ClientMsg>(r#"{"op":"nope"}"#).is_err());
    }

    #[test]
    fn parses_reply_ok_and_err() {
        let ok: ClientMsg =
            serde_json::from_str(r#"{"op":"reply","reqId":"g7","ok":true,"value":42}"#).unwrap();
        assert_eq!(
            ok,
            ClientMsg::Reply {
                req_id: "g7".into(),
                ok: true,
                value: serde_json::json!(42),
                error: None
            }
        );
        let err: ClientMsg =
            serde_json::from_str(r#"{"op":"reply","reqId":"g8","ok":false,"error":"boom"}"#)
                .unwrap();
        assert_eq!(
            err,
            ClientMsg::Reply {
                req_id: "g8".into(),
                ok: false,
                value: serde_json::Value::Null,
                error: Some("boom".into())
            }
        );
    }

    #[test]
    fn parses_emit() {
        let m: ClientMsg =
            serde_json::from_str(r#"{"op":"emit","handle":"H","event":"tick","payload":{"n":1}}"#)
                .unwrap();
        assert_eq!(
            m,
            ClientMsg::Emit {
                handle: "H".into(),
                event: "tick".into(),
                payload: serde_json::json!({"n":1})
            }
        );
    }

    #[test]
    fn parses_focus_with_handle() {
        let m: ClientMsg =
            serde_json::from_str(r#"{"op":"focus","handle":"h1","instance":null}"#).unwrap();
        assert_eq!(
            m,
            ClientMsg::Focus {
                handle: Some("h1".into()),
                instance: None,
            }
        );
    }

    #[test]
    fn parses_blur_with_null_handle() {
        let m: ClientMsg = serde_json::from_str(r#"{"op":"focus","handle":null}"#).unwrap();
        assert_eq!(
            m,
            ClientMsg::Focus {
                handle: None,
                instance: None,
            }
        );
    }

    #[test]
    fn parses_register_with_forward_keys() {
        let m: ClientMsg = serde_json::from_str(
            r#"{"op":"register","kind":"inline","html":"x","forward_keys":[{"mods":["alt"],"key":"h"}]}"#,
        )
        .unwrap();
        match m {
            ClientMsg::Register(RegisterKind::Inline { forward_keys, .. }) => {
                assert_eq!(forward_keys.len(), 1);
                assert_eq!(forward_keys[0].key, "h");
                assert_eq!(forward_keys[0].mods, vec!["alt".to_string()]);
            }
            _ => panic!("expected inline register"),
        }
    }

    #[test]
    fn parses_url_register_with_defaults() {
        let m: ClientMsg =
            serde_json::from_str(r#"{"op":"register","kind":"url","url":"https://example.com"}"#)
                .unwrap();
        assert_eq!(
            m,
            ClientMsg::Register(RegisterKind::Url {
                url: "https://example.com".into(),
                interactive: true,
                bridge: false,
                forward_keys: vec![],
                preload: vec![],
            })
        );
    }

    #[test]
    fn parses_url_register_with_bridge_true() {
        let m: ClientMsg = serde_json::from_str(
            r#"{"op":"register","kind":"url","url":"https://app.example.com","bridge":true}"#,
        )
        .unwrap();
        assert_eq!(
            m,
            ClientMsg::Register(RegisterKind::Url {
                url: "https://app.example.com".into(),
                interactive: true,
                bridge: true,
                forward_keys: vec![],
                preload: vec![],
            })
        );
    }

    #[test]
    fn host_parses_the_exact_wire_string_the_sdk_emits() {
        let wire = r#"{"op":"register","kind":"url","url":"https://example.com","interactive":true,"bridge":false}"#;
        let m: ClientMsg = serde_json::from_str(wire).unwrap();
        assert_eq!(
            m,
            ClientMsg::Register(RegisterKind::Url {
                url: "https://example.com".into(),
                interactive: true,
                bridge: false,
                forward_keys: vec![],
                preload: vec![],
            })
        );
    }

    #[test]
    fn serializes_ok_and_err() {
        assert_eq!(
            serde_json::to_string(&ServerMsg::ok("h1")).unwrap(),
            r#"{"ok":true,"handle":"h1"}"#
        );
        assert_eq!(
            serde_json::to_string(&ServerMsg::err("invalid_root")).unwrap(),
            r#"{"ok":false,"error":"invalid_root"}"#
        );
    }

    #[test]
    fn serializes_compositing_start() {
        let msg = PushMsg::Compositing {
            handle: "abc123".into(),
            active: true,
        };
        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            r#"{"op":"compositing","handle":"abc123","active":true}"#
        );
    }

    #[test]
    fn serializes_compositing_stop() {
        let msg = PushMsg::Compositing {
            handle: "abc123".into(),
            active: false,
        };
        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            r#"{"op":"compositing","handle":"abc123","active":false}"#
        );
    }

    #[test]
    fn parses_navigate_back() {
        let m: ClientMsg =
            serde_json::from_str(r#"{"op":"navigate","handle":"H","action":"back"}"#).unwrap();
        assert_eq!(
            m,
            ClientMsg::Navigate {
                handle: "H".into(),
                action: NavAction::Back,
            }
        );
    }

    #[test]
    fn parses_navigate_to_url() {
        let m: ClientMsg = serde_json::from_str(
            r#"{"op":"navigate","handle":"H","action":{"to":"https://example.com"}}"#,
        )
        .unwrap();
        assert_eq!(
            m,
            ClientMsg::Navigate {
                handle: "H".into(),
                action: NavAction::To("https://example.com".into()),
            }
        );
    }

    #[test]
    fn parses_register_with_preload() {
        let m: ClientMsg = serde_json::from_str(
            r#"{"op":"register","kind":"inline","html":"x","preload":["window.A=1;"]}"#,
        )
        .unwrap();
        match m {
            ClientMsg::Register(RegisterKind::Inline { preload, .. }) => {
                assert_eq!(preload, vec!["window.A=1;".to_string()]);
            }
            _ => panic!("expected inline register"),
        }
    }

    #[test]
    fn preload_defaults_empty_when_absent() {
        let m: ClientMsg =
            serde_json::from_str(r#"{"op":"register","kind":"inline","html":"x"}"#).unwrap();
        match m {
            ClientMsg::Register(RegisterKind::Inline { preload, .. }) => {
                assert!(preload.is_empty())
            }
            _ => panic!("expected inline register"),
        }
    }
}
