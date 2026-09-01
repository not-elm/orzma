//! Control-socket NDJSON wire types.

use crate::keychord::KeyChord;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Borrow;
use std::fmt;

/// The opaque identity of one registration, as minted by the control plane.
///
/// A handle addresses the registration itself — the unit `emit` fans out over
/// and `new_instance` mints from. Mounting and navigation are addressed by an
/// instance id instead, which [`crate::WebviewHandle::instance_id`] returns.
///
/// # Invariants
///
/// There is deliberately no `From<HandleId> for String`. That absence is what
/// makes `WebviewWidget::new(handle.handle_id())` a compile error instead of a
/// webview that silently never appears; adding the conversion for convenience
/// reopens that path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HandleId(String);

impl HandleId {
    /// Borrows the wire spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for HandleId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl fmt::Display for HandleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Borrow<str> for HandleId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// A message the SDK writes to the control socket.
// NOTE: rename_all must stay snake_case so `NewInstance` renders as the host's
// `new_instance`; under lowercase it would silently become an unknown op the
// host rejects.
#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum ClientMsg {
    /// Handshake with `$ORZMA_TOKEN`.
    Hello {
        /// The pane's `$ORZMA_TOKEN`.
        token: String,
    },
    /// Register content; the reply mints a handle and its first instance.
    Register(RegisterKind),
    /// Mint an additional placement instance on a handle this connection owns.
    NewInstance {
        /// The handle returned by a prior `register`.
        handle: HandleId,
    },
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
        handle: HandleId,
        /// The event name routed to `window.orzma.on(event, …)`.
        event: String,
        /// The event payload.
        payload: Value,
    },
    /// Sets (or clears) the app-owned focus target. `instance: None` blurs any
    /// focused webview back to the app (native widget).
    Focus {
        /// The placement to focus, or `None` to blur.
        instance: Option<String>,
    },
    /// Navigate one mounted placement in place (no re-registration).
    Navigate {
        /// The target placement.
        instance: String,
        /// What to do.
        action: NavAction,
    },
}

/// A navigation action on one mounted placement.
// NOTE: rename_all must match the host's NavAction (snake_case) so the wire
// contract agrees for any future multi-word variant, not just the current
// single-word ones where lowercase and snake_case coincide.
#[derive(Debug, Serialize)]
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
        /// User-supplied scripts injected before the page's own scripts run.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        preload: Vec<String>,
    },
    /// A directory of assets served at `orzma://<handle>/`.
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
        /// User-supplied scripts injected before the page's own scripts run.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        preload: Vec<String>,
    },
    /// Load a remote `http(s)` URL as the top-level document.
    Url {
        /// The `http(s)` URL to load.
        url: String,
        /// Whether the view accepts focus/input.
        interactive: bool,
        /// Whether the `window.orzma` back-channel is injected (opt-in).
        bridge: bool,
        /// Chords the page lets through to the app while focused.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        forward_keys: Vec<KeyChord>,
        /// User-supplied scripts injected before the page's own scripts run.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        preload: Vec<String>,
    },
}

/// The untagged reply to a `register` or `new_instance` request.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct ServerReply {
    /// Whether the request succeeded.
    pub ok: bool,
    /// The minted handle, present on a successful `register`.
    #[serde(default)]
    pub handle: Option<HandleId>,
    /// The minted instance, present on either successful reply.
    #[serde(default)]
    pub instance: Option<String>,
    /// The error string, present when `!ok`.
    #[serde(default)]
    pub error: Option<String>,
}

/// An inbound `call` frame forwarded from a page's `window.orzma.call`.
#[derive(Debug, Deserialize)]
pub(crate) struct IncomingCall {
    /// The registration the call targets.
    pub handle: String,
    /// The placement the calling page is mounted in.
    pub instance: String,
    /// The global request id to echo in the reply.
    #[serde(rename = "reqId")]
    pub req_id: Value,
    /// The invoked method name.
    pub method: String,
    /// The single params value (any JSON shape; absent deserializes as null).
    #[serde(default)]
    pub params: Value,
}

/// An inbound one-way `event` frame forwarded from a page's `window.orzma.emit`.
#[derive(Debug, Deserialize)]
pub(crate) struct IncomingEvent {
    /// The view handle the event targets.
    pub(crate) handle: String,
    /// The declared event name (`add_event::<T>(name)`).
    pub(crate) event: String,
    /// The single payload value (any JSON shape; absent deserializes as null).
    #[serde(default)]
    pub(crate) payload: Value,
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
            preload: Vec::new(),
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

    /// Asserts that one reply type covers both minting replies and the
    /// rejection, with each absent field deserializing as `None`.
    ///
    /// Case: a program registers a view, asks that registration for a second
    /// placement, and then asks for one more against a handle it does not own.
    #[test]
    fn server_reply_deserializes_both_mints_and_the_rejection() {
        let registered: ServerReply =
            serde_json::from_str(r#"{"ok":true,"handle":"abc","instance":"i1"}"#).unwrap();
        assert_eq!(registered.handle.map(|h| h.to_string()), Some("abc".into()));
        assert_eq!(registered.instance.as_deref(), Some("i1"));

        let instanced: ServerReply =
            serde_json::from_str(r#"{"ok":true,"instance":"i2"}"#).unwrap();
        assert!(instanced.handle.is_none());
        assert_eq!(instanced.instance.as_deref(), Some("i2"));

        let rejected: ServerReply =
            serde_json::from_str(r#"{"ok":false,"error":"unknown_handle"}"#).unwrap();
        assert!(!rejected.ok);
        assert_eq!(rejected.error.as_deref(), Some("unknown_handle"));
    }

    /// Asserts that a `new_instance` request renders under the host's
    /// snake_case op name, carrying the handle it mints from.
    ///
    /// Case: an app that already registered a view asks for a second
    /// placement so it can show that view twice in a split.
    #[test]
    fn new_instance_serializes_under_its_snake_case_op() {
        let v = serde_json::to_value(ClientMsg::NewInstance {
            handle: "H".to_owned().into(),
        })
        .unwrap();
        assert_eq!(v["op"], "new_instance");
        assert_eq!(v["handle"], "H");
    }

    /// Asserts that an inbound call carries the placement it came from
    /// alongside the registration it targets.
    ///
    /// Case: a page mounted in one of two placements of a view calls back into
    /// the app, and the handler needs to know which one asked.
    #[test]
    fn call_deserializes() {
        let c: IncomingCall = serde_json::from_str(
            r#"{"op":"call","handle":"h","instance":"i1","reqId":"3","method":"ping","params":"x"}"#,
        )
        .unwrap();
        assert_eq!(c.handle, "h");
        assert_eq!(c.instance, "i1");
        assert_eq!(c.method, "ping");
        assert_eq!(c.params, serde_json::json!("x"));
    }

    /// Asserts that an omitted `params` deserializes as null rather than
    /// failing the whole frame.
    ///
    /// Case: a page calls a method that takes no argument.
    #[test]
    fn call_without_params_deserializes_as_null() {
        let c: IncomingCall = serde_json::from_str(
            r#"{"op":"call","handle":"h","instance":"i1","reqId":"3","method":"ping"}"#,
        )
        .unwrap();
        assert_eq!(c.params, Value::Null);
    }

    /// Asserts that a focus op names the placement it focuses.
    ///
    /// Case: the app moves focus into one of two side-by-side placements of
    /// the same registration.
    #[test]
    fn focus_serializes_with_instance() {
        let line = serde_json::to_string(&ClientMsg::Focus {
            instance: Some("i1".into()),
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["instance"], "i1");
    }

    /// Asserts that a blur renders as a focus op with a null instance.
    ///
    /// Case: the user tabs out of a webview and back to a native widget.
    #[test]
    fn blur_serializes_with_null_instance() {
        let line = serde_json::to_string(&ClientMsg::Focus { instance: None }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["instance"], serde_json::Value::Null);
    }

    #[test]
    fn register_url_serializes() {
        let v = serde_json::to_value(ClientMsg::Register(RegisterKind::Url {
            url: "https://example.com".into(),
            interactive: true,
            bridge: false,
            forward_keys: Vec::new(),
            preload: Vec::new(),
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
            preload: Vec::new(),
        }))
        .unwrap();
        assert_eq!(v["kind"], "url");
        assert_eq!(v["bridge"], true);
    }

    /// Asserts that a navigation names the placement it drives, not the
    /// registration behind it.
    ///
    /// Case: the user presses the back key while one of two placements of a
    /// browser view is focused.
    #[test]
    fn navigate_back_serializes() {
        let v = serde_json::to_value(ClientMsg::Navigate {
            instance: "i1".into(),
            action: NavAction::Back,
        })
        .unwrap();
        assert_eq!(v["op"], "navigate");
        assert_eq!(v["instance"], "i1");
        assert_eq!(v["action"], "back");
    }

    /// Asserts that a URL navigation carries its target under the `to` key.
    ///
    /// Case: the app follows a link by driving one placement to a new URL.
    #[test]
    fn navigate_to_serializes_url_under_to() {
        let v = serde_json::to_value(ClientMsg::Navigate {
            instance: "i1".into(),
            action: NavAction::To("https://example.com/x".into()),
        })
        .unwrap();
        assert_eq!(v["op"], "navigate");
        assert_eq!(v["action"]["to"], "https://example.com/x");
    }

    #[test]
    fn incoming_event_deserializes() {
        let e: IncomingEvent = serde_json::from_str(
            r#"{"op":"event","handle":"h","event":"hello","payload":{"message":"hi"}}"#,
        )
        .unwrap();
        assert_eq!(e.handle, "h");
        assert_eq!(e.event, "hello");
        assert_eq!(e.payload, serde_json::json!({"message":"hi"}));
    }

    #[test]
    fn incoming_event_without_payload_is_null() {
        let e: IncomingEvent =
            serde_json::from_str(r#"{"op":"event","handle":"h","event":"ping"}"#).unwrap();
        assert_eq!(e.payload, Value::Null);
    }
}
