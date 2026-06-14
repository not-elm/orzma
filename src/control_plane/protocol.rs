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
        /// The per-surface `$OZMUX_TOKEN` value.
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
        /// The event name dispatched to page `window.ozmux.on(name, …)`.
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
    },
    /// Serve a single dynamic HTML document supplied inline.
    Inline {
        /// The full HTML document.
        html: String,
        /// Whether the mounted webview accepts pointer/keyboard input.
        #[serde(default = "default_true")]
        interactive: bool,
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
        /// The opaque handle to mount via `OSC mount-inline;<handle>`.
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
}
