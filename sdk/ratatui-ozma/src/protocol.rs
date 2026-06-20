//! Control-socket NDJSON wire types.

use crate::keychord::KeyChord;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A message the SDK writes to the control socket.
#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub(crate) enum ClientMsg {
    /// Handshake with `$OZMA_TOKEN`.
    Hello {
        /// The pane's `$OZMA_TOKEN`.
        token: String,
    },
    /// Register content; the reply mints a handle.
    Register(RegisterKind),
    /// Reply to an inbound `call`.
    Reply {
        /// Echoes the inbound `reqId` verbatim.
        #[serde(rename = "reqId")]
        req_id: Value,
        /// The handler outcome, flattened into `ok`/`value`/`error`.
        #[serde(flatten, with = "reply_result")]
        result: Result<Value, String>,
    },
    /// Push an event to the mounted page(s) of a handle.
    Emit {
        /// The target handle.
        handle: String,
        /// The event name routed to `window.ozma.on(event, …)`.
        event: String,
        /// The event payload.
        payload: Value,
    },
    /// Sets (or clears) the app-owned focus target. `handle: None` blurs any
    /// focused webview back to the app (native widget).
    Focus {
        /// The webview handle to focus, or `None` to blur.
        handle: Option<String>,
        /// The mount instance id, or `None` for the default instance.
        instance: Option<String>,
    },
}

/// The content variants of a `register` request.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum RegisterKind {
    /// A full inline HTML document.
    Inline {
        /// The HTML document.
        html: String,
        /// Whether the view accepts focus/input.
        interactive: bool,
        /// Chords the page lets through to the app while focused.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        forward_keys: Vec<KeyChord>,
    },
    /// A directory of assets served at `ozma-dyn://<handle>/`.
    Dir {
        /// Absolute asset root.
        root: String,
        /// Entry HTML path relative to `root`.
        entry: String,
        /// Whether the view accepts focus/input.
        interactive: bool,
        /// Chords the page lets through to the app while focused.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        forward_keys: Vec<KeyChord>,
    },
    /// Load a remote `http(s)` URL as the top-level document.
    Url {
        /// The `http(s)` URL to load.
        url: String,
        /// Whether the view accepts focus/input.
        interactive: bool,
        /// Whether the `window.ozma` back-channel is injected (opt-in).
        bridge: bool,
        /// Chords the page lets through to the app while focused.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        forward_keys: Vec<KeyChord>,
    },
}

/// The untagged reply to a `register` request.
#[derive(Debug, Deserialize)]
pub(crate) struct RegisterReply {
    /// Whether registration succeeded.
    pub(crate) ok: bool,
    /// The minted handle, present when `ok`.
    #[serde(default)]
    pub(crate) handle: Option<String>,
    /// The error string, present when `!ok`.
    #[serde(default)]
    pub(crate) error: Option<String>,
}

/// An inbound `call` frame forwarded from a page's `window.ozma.call`.
#[derive(Debug, Deserialize)]
pub(crate) struct IncomingCall {
    /// The view handle the call targets.
    pub(crate) handle: String,
    /// The global request id to echo in the reply.
    #[serde(rename = "reqId")]
    pub(crate) req_id: Value,
    /// The invoked method name.
    pub(crate) method: String,
    /// The single params value (any JSON shape; absent deserializes as null).
    #[serde(default)]
    pub(crate) params: Value,
}

mod reply_result {
    use serde::Serializer;
    use serde::ser::SerializeMap;
    use serde_json::Value;

    pub(super) fn serialize<S>(result: &Result<Value, String>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = s.serialize_map(None)?;
        match result {
            Ok(value) => {
                map.serialize_entry("ok", &true)?;
                map.serialize_entry("value", value)?;
            }
            Err(error) => {
                map.serialize_entry("ok", &false)?;
                map.serialize_entry("error", error)?;
            }
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hello_serializes() {
        let line = serde_json::to_string(&ClientMsg::Hello { token: "T".into() }).unwrap();
        assert_eq!(line, r#"{"op":"hello","token":"T"}"#);
    }

    #[test]
    fn register_inline_serializes() {
        let v = serde_json::to_value(ClientMsg::Register(RegisterKind::Inline {
            html: "<h1>hi</h1>".into(),
            interactive: true,
            forward_keys: Vec::new(),
        }))
        .unwrap();
        assert_eq!(v["op"], "register");
        assert_eq!(v["kind"], "inline");
        assert_eq!(v["html"], "<h1>hi</h1>");
        assert_eq!(v["interactive"], true);
    }

    #[test]
    fn reply_ok_serializes() {
        let v = serde_json::to_value(ClientMsg::Reply {
            req_id: json!("17"),
            result: Ok(json!("pong")),
        })
        .unwrap();
        assert_eq!(v["op"], "reply");
        assert_eq!(v["reqId"], "17");
        assert_eq!(v["ok"], true);
        assert_eq!(v["value"], "pong");
    }

    #[test]
    fn reply_err_serializes() {
        let v = serde_json::to_value(ClientMsg::Reply {
            req_id: json!("9"),
            result: Err("unknown_method".into()),
        })
        .unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "unknown_method");
    }

    #[test]
    fn register_reply_deserializes_ok_and_err() {
        let ok: RegisterReply = serde_json::from_str(r#"{"ok":true,"handle":"abc"}"#).unwrap();
        assert_eq!(ok.handle.as_deref(), Some("abc"));
        let err: RegisterReply =
            serde_json::from_str(r#"{"ok":false,"error":"unsafe_entry"}"#).unwrap();
        assert!(!err.ok);
        assert_eq!(err.error.as_deref(), Some("unsafe_entry"));
    }

    #[test]
    fn call_deserializes() {
        let c: IncomingCall = serde_json::from_str(
            r#"{"op":"call","handle":"h","reqId":"3","method":"ping","params":"x"}"#,
        )
        .unwrap();
        assert_eq!(c.handle, "h");
        assert_eq!(c.method, "ping");
        assert_eq!(c.params, serde_json::json!("x"));
    }

    #[test]
    fn call_without_params_deserializes_as_null() {
        let c: IncomingCall =
            serde_json::from_str(r#"{"op":"call","handle":"h","reqId":"3","method":"ping"}"#)
                .unwrap();
        assert_eq!(c.params, Value::Null);
    }

    #[test]
    fn focus_serializes_with_handle() {
        let line = serde_json::to_string(&ClientMsg::Focus {
            handle: Some("h1".into()),
            instance: None,
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["handle"], "h1");
        assert_eq!(v["instance"], serde_json::Value::Null);
    }

    #[test]
    fn blur_serializes_with_null_handle() {
        let line = serde_json::to_string(&ClientMsg::Focus {
            handle: None,
            instance: None,
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["handle"], serde_json::Value::Null);
    }

    #[test]
    fn register_url_serializes() {
        let v = serde_json::to_value(ClientMsg::Register(RegisterKind::Url {
            url: "https://example.com".into(),
            interactive: true,
            bridge: false,
            forward_keys: Vec::new(),
        }))
        .unwrap();
        assert_eq!(v["op"], "register");
        assert_eq!(v["kind"], "url");
        assert_eq!(v["url"], "https://example.com");
        assert_eq!(v["interactive"], true);
        assert_eq!(v["bridge"], false);
        assert!(
            v.get("forward_keys").is_none(),
            "empty forward_keys must be skipped"
        );
    }

    #[test]
    fn register_url_serializes_bridge_true() {
        let v = serde_json::to_value(ClientMsg::Register(RegisterKind::Url {
            url: "https://app.example.com".into(),
            interactive: true,
            bridge: true,
            forward_keys: Vec::new(),
        }))
        .unwrap();
        assert_eq!(v["kind"], "url");
        assert_eq!(v["bridge"], true);
    }
}
