//! NDJSON wire types for the control plane, internally tagged on `op`. The
//! listener parses one `ClientMsg` per line and replies with one `ServerMsg`
//! per request line. Unknown `op` values fail to parse (strict, matching the
//! OSC parser ethos).

use crate::control_plane::HandleId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One inbound control-plane request line.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum ClientMsg {
    /// Connection handshake: binds this connection to the pane whose env
    /// carries `token`. Sent once, before any `register`.
    Hello {
        /// The per-surface `$ORZMA_TOKEN` value.
        token: String,
    },
    /// Registers a dynamic view and requests a handle.
    Register(RegisterKind),
    /// Releases a previously-registered handle owned by this connection.
    Unregister {
        /// The handle returned by a prior `register`.
        handle: HandleId,
    },
    /// Mints an additional placement slot for a handle this connection owns.
    NewInstance {
        /// The handle returned by a prior `register`.
        handle: HandleId,
    },
    /// A program's reply to an orzma-initiated `call` (back-channel).
    Reply {
        /// The global reqId orzma assigned to the originating `call`.
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
        handle: HandleId,
        /// The event name dispatched to page `window.orzma.on(name, …)`.
        event: String,
        /// The event payload.
        #[serde(default)]
        payload: Value,
    },
    /// Sets (or clears, with `instance: None`) the app-owned focus target
    /// for this connection's surface.
    Focus {
        /// The instance to focus, or `None` to blur.
        #[serde(default)]
        instance: Option<String>,
    },
    /// Navigate one mounted placement in place.
    Navigate {
        /// The target instance.
        instance: String,
        /// What to do.
        action: NavAction,
    },
}

/// A navigation action on one mounted placement.
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
        /// Whether the `window.orzma` back-channel is injected (opt-in).
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
    /// A successful `register`: the minted handle and its first instance.
    Registered {
        /// Always `true`.
        ok: bool,
        /// The opaque handle the registration is owned and released by.
        handle: HandleId,
        /// The instance to mount via `APC Omount;n=<instance>`.
        instance: String,
    },
    /// A successful `new_instance`: the minted instance.
    Instanced {
        /// Always `true`.
        ok: bool,
        /// The instance to mount via `APC Omount;n=<instance>`.
        instance: String,
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
    /// A `register` reply carrying the minted handle and its first instance.
    pub fn registered(handle: impl Into<HandleId>, instance: impl Into<String>) -> Self {
        Self::Registered {
            ok: true,
            handle: handle.into(),
            instance: instance.into(),
        }
    }

    /// A `new_instance` reply carrying the minted instance.
    pub fn instanced(instance: impl Into<String>) -> Self {
        Self::Instanced {
            ok: true,
            instance: instance.into(),
        }
    }

    /// An error reply carrying a short code.
    pub fn err(error: impl Into<String>) -> Self {
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
        handle: HandleId,
        /// The placement instance whose compositing state changed.
        instance: String,
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

    /// Asserts that a `new_instance` line parses into its own variant
    /// carrying the handle the extra placement is minted under.
    ///
    /// Case: a program that already registered a view asks for a second
    /// placement slot before writing its mount.
    #[test]
    fn parses_new_instance() {
        let m: ClientMsg = serde_json::from_str(r#"{"op":"new_instance","handle":"h1"}"#).unwrap();
        assert_eq!(
            m,
            ClientMsg::NewInstance {
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

    /// Asserts that a `focus` line addresses one instance, and that a null
    /// instance parses as the blur request.
    ///
    /// Case: an app moves keyboard focus onto one of its two mounted
    /// placements, then hands focus back to the terminal.
    #[test]
    fn parses_focus_and_blur_by_instance() {
        let focus: ClientMsg = serde_json::from_str(r#"{"op":"focus","instance":"3f5a"}"#).unwrap();
        assert_eq!(
            focus,
            ClientMsg::Focus {
                instance: Some("3f5a".into())
            }
        );
        let blur: ClientMsg = serde_json::from_str(r#"{"op":"focus","instance":null}"#).unwrap();
        assert_eq!(blur, ClientMsg::Focus { instance: None });
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

    /// Asserts that each of the three reply shapes serializes to the exact
    /// line the SDK's position-matched FIFO reads back.
    ///
    /// Case: one connection registers a view, asks for a second placement,
    /// then sends a request the host rejects.
    #[test]
    fn serializes_the_three_reply_shapes() {
        assert_eq!(
            serde_json::to_string(&ServerMsg::registered("h1", "i1")).unwrap(),
            r#"{"ok":true,"handle":"h1","instance":"i1"}"#
        );
        assert_eq!(
            serde_json::to_string(&ServerMsg::instanced("i2")).unwrap(),
            r#"{"ok":true,"instance":"i2"}"#
        );
        assert_eq!(
            serde_json::to_string(&ServerMsg::err("unknown_handle")).unwrap(),
            r#"{"ok":false,"error":"unknown_handle"}"#
        );
    }

    /// Asserts that a compositing push names the placement it is about, not
    /// only the registration it came from.
    ///
    /// Case: a program holding two placements of one handle is told that
    /// the second of them started painting.
    #[test]
    fn serializes_compositing_with_its_instance() {
        let msg = PushMsg::Compositing {
            handle: "abc123".into(),
            instance: "i1".into(),
            active: true,
        };
        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            r#"{"op":"compositing","handle":"abc123","instance":"i1","active":true}"#
        );
    }

    /// Asserts that a `navigate` line addresses one instance and carries a
    /// history action.
    ///
    /// Case: an embedded browser UI sends Back for the placement the user
    /// is looking at.
    #[test]
    fn parses_navigate_by_instance() {
        let m: ClientMsg =
            serde_json::from_str(r#"{"op":"navigate","instance":"3f5a","action":"back"}"#).unwrap();
        assert_eq!(
            m,
            ClientMsg::Navigate {
                instance: "3f5a".into(),
                action: NavAction::Back,
            }
        );
    }

    /// Asserts that the `to` action carries its target URL through the
    /// instance-addressed navigate line.
    ///
    /// Case: an embedded browser UI loads a new page into the placement
    /// the user typed the address for.
    #[test]
    fn parses_navigate_to_url() {
        let m: ClientMsg = serde_json::from_str(
            r#"{"op":"navigate","instance":"3f5a","action":{"to":"https://example.com"}}"#,
        )
        .unwrap();
        assert_eq!(
            m,
            ClientMsg::Navigate {
                instance: "3f5a".into(),
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
                assert!(preload.is_empty());
            }
            _ => panic!("expected inline register"),
        }
    }
}
