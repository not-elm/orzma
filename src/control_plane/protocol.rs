//! NDJSON wire types for the control plane, internally tagged on `op`. The
//! listener parses one `ClientMsg` per line and replies with one `ServerMsg`
//! per request line. Unknown `op` values fail to parse (strict, matching the
//! OSC parser ethos).

use serde::{Deserialize, Serialize};

/// One inbound control-plane request line.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
