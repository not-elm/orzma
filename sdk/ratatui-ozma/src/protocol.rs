//! Control-socket NDJSON wire types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A message the SDK writes to the control socket.
#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub(crate) enum ClientMsg {
    /// Handshake with `$OZMUX_TOKEN`.
    Hello {
        /// The pane's `$OZMUX_TOKEN`.
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
        /// The event name routed to `window.ozmux.on(event, …)`.
        event: String,
        /// The event payload.
        payload: Value,
    },
}

/// The content variants of a `register` request.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum RegisterKind {
    /// A full inline HTML document.
    Inline {
        /// The HTML document.
        html: String,
        /// Whether the view accepts focus/input.
        interactive: bool,
    },
    /// A directory of assets served at `ozmux-dyn://<handle>/`.
    Dir {
        /// Absolute asset root.
        root: String,
        /// Entry HTML path relative to `root`.
        entry: String,
        /// Whether the view accepts focus/input.
        interactive: bool,
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

/// An inbound `call` frame forwarded from a page's `window.ozmux.call`.
#[derive(Debug, Deserialize)]
pub(crate) struct IncomingCall {
    /// The view handle the call targets.
    pub(crate) handle: String,
    /// The global request id to echo in the reply.
    #[serde(rename = "reqId")]
    pub(crate) req_id: Value,
    /// The invoked method name.
    pub(crate) method: String,
    /// The positional arguments array.
    #[serde(default)]
    pub(crate) args: Vec<Value>,
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
            r#"{"op":"call","handle":"h","reqId":"3","method":"ping","args":["x"]}"#,
        )
        .unwrap();
        assert_eq!(c.handle, "h");
        assert_eq!(c.method, "ping");
        assert_eq!(c.args, vec![serde_json::json!("x")]);
    }
}
