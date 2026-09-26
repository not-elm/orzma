//! The control socket's NDJSON wire types, internally tagged on `op`: one
//! `ClientMsg` per inbound line, one `ServerMsg` per request line, and a
//! `PushMsg` for each line the host sends unasked.

use crate::boundary::{ForwardChord, HandleId};
use orzma_vt::prelude::InstanceId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One inbound control-plane line. An unknown `op` fails to parse.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Connection handshake: binds this connection to the pane whose env
    /// carries `token`. Sent once, before any other line.
    Hello {
        /// The per-pane `$ORZMA_TOKEN` value.
        token: String,
    },
    /// Registers a dynamic view and requests a handle.
    Register(RegisterKind),
    /// Releases a handle this connection registered.
    Unregister {
        /// The handle returned by a prior `register`.
        handle: HandleId,
    },
    /// Mints an additional placement for a handle this connection owns.
    NewInstance {
        /// The handle returned by a prior `register`.
        handle: HandleId,
    },
    /// A program's reply to a host-initiated `call`.
    Reply {
        /// The global reqId the host assigned to the `call`.
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
    /// A program-initiated event for its handle's mounted pages.
    Emit {
        /// The handle whose mounted pages receive the event.
        handle: HandleId,
        /// The event name dispatched to `window.orzma.on(name, …)`.
        event: String,
        /// The event payload.
        #[serde(default)]
        payload: Value,
    },
    /// Focuses one placement, or with `null` releases the focus held in
    /// this connection's pane.
    Focus {
        /// The instance to focus, or `None` to blur.
        #[serde(default)]
        instance: Option<String>,
    },
    /// Navigates one mounted placement in place.
    Navigate {
        /// The target instance.
        instance: String,
        /// What to do.
        action: NavAction,
    },
    /// Mounts a placement at a visible cell of this connection's pane: the
    /// socket counterpart of the APC `mount`, for PTYs that drop APC.
    Mount {
        /// The target instance.
        instance: String,
        /// 0-based visible row of the rect's top edge.
        row: u16,
        /// 0-based column of the rect's left edge.
        col: u16,
        /// Rect height in cells (`1..=MAX_ROWS`).
        rows: u16,
        /// Rect width in cells (`1..=MAX_COLS`).
        cols: u16,
    },
    /// Removes one placement this connection mounted.
    Unmount {
        /// The target instance.
        instance: String,
    },
    /// Replaces the forward-key chords of a handle this connection owns.
    SetForwardKeys {
        /// The handle returned by a prior `register`.
        handle: HandleId,
        /// The complete new chord list.
        keys: Vec<ForwardChord>,
    },
}

/// A navigation action on one mounted placement.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavAction {
    /// Go back in the webview's session history.
    Back,
    /// Go forward in the webview's session history.
    Forward,
    /// Reload the current page.
    Reload,
    /// Navigate to a new URL.
    To(String),
}

/// The content source a `register` declares.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegisterKind {
    /// Serve files under `root` (absolute) at entry `entry`.
    Dir {
        /// Absolute asset root directory.
        root: String,
        /// HTML entry path relative to `root`.
        entry: String,
        /// Whether the page accepts pointer and keyboard input.
        #[serde(default = "default_true")]
        interactive: bool,
        /// Chords the host passes to the PTY instead of the page.
        #[serde(default)]
        forward_keys: Vec<ForwardChord>,
        /// Scripts injected before the page's own scripts run.
        #[serde(default)]
        preload: Vec<String>,
    },
    /// Serve one HTML document supplied inline.
    Inline {
        /// The full HTML document.
        html: String,
        /// Whether the page accepts pointer and keyboard input.
        #[serde(default = "default_true")]
        interactive: bool,
        /// Chords the host passes to the PTY instead of the page.
        #[serde(default)]
        forward_keys: Vec<ForwardChord>,
        /// Scripts injected before the page's own scripts run.
        #[serde(default)]
        preload: Vec<String>,
    },
    /// Load a remote `http(s)` URL as the top-level document.
    Url {
        /// The `http(s)` URL to load.
        url: String,
        /// Whether the page accepts pointer and keyboard input.
        #[serde(default = "default_true")]
        interactive: bool,
        /// Whether the `window.orzma` bridge is injected (opt-in).
        #[serde(default)]
        bridge: bool,
        /// Chords the host passes to the PTY instead of the page.
        #[serde(default)]
        forward_keys: Vec<ForwardChord>,
        /// Scripts injected before the page's own scripts run.
        #[serde(default)]
        preload: Vec<String>,
    },
}

/// One outbound reply line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ServerMsg {
    /// A successful `register`: the minted handle and its first instance.
    Registered {
        /// Always `true`.
        ok: bool,
        /// The handle the registration is owned and released by.
        handle: HandleId,
        /// The first instance, in its 32-hex-digit wire spelling.
        instance: String,
    },
    /// A successful `new_instance`: the minted instance.
    Instanced {
        /// Always `true`.
        ok: bool,
        /// The instance, in its wire spelling.
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
    pub fn registered(handle: impl Into<HandleId>, instance: InstanceId) -> Self {
        Self::Registered {
            ok: true,
            handle: handle.into(),
            instance: instance.to_string(),
        }
    }

    /// A `new_instance` reply carrying the minted instance.
    pub fn instanced(instance: InstanceId) -> Self {
        Self::Instanced {
            ok: true,
            instance: instance.to_string(),
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

/// An outbound line the host sends a program without being asked.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PushMsg {
    /// A placement first composited (`active: true`), or was unmounted
    /// after compositing (`active: false`).
    Compositing {
        /// The registration the placement belongs to.
        handle: HandleId,
        /// The placement, in its wire spelling.
        instance: String,
        /// `true` when compositing starts; `false` when it stops.
        active: bool,
    },
    /// A placement gained (`focused: true`) or lost (`focused: false`)
    /// webview keyboard focus.
    FocusChanged {
        /// The registration the placement belongs to.
        handle: HandleId,
        /// The placement, in its wire spelling.
        instance: String,
        /// Whether the placement now holds webview focus.
        focused: bool,
    },
    /// A page called `window.orzma.call(method, params)`, or the host
    /// reports a `urlChanged`; the program answers with a `reply` carrying
    /// `req_id`.
    Call {
        /// The registration whose page called.
        handle: HandleId,
        /// The placement whose page called, in its wire spelling.
        instance: String,
        /// The global reqId the reply must carry.
        #[serde(rename = "reqId")]
        req_id: String,
        /// The method name.
        method: String,
        /// The call parameters.
        params: Value,
    },
    /// A page called `window.orzma.emit(event, payload)`.
    Event {
        /// The registration whose page emitted.
        handle: HandleId,
        /// The event name.
        event: String,
        /// The event payload.
        payload: Value,
    },
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(line: &str) -> ClientMsg {
        serde_json::from_str(line).expect("a valid client line")
    }

    /// Asserts that a `hello` line parses into its token.
    ///
    /// Case: a program starts in a pane and opens the control socket.
    #[test]
    fn parses_hello() {
        assert_eq!(
            parse(r#"{"op":"hello","token":"t1"}"#),
            ClientMsg::Hello { token: "t1".into() }
        );
    }

    /// Asserts that a `dir` register without `interactive` defaults to an
    /// interactive view with no chords and no preload.
    ///
    /// Case: a program registers its bundle directory with the minimal
    /// payload.
    #[test]
    fn parses_dir_register_with_defaults() {
        assert_eq!(
            parse(r#"{"op":"register","kind":"dir","root":"/abs","entry":"index.html"}"#),
            ClientMsg::Register(RegisterKind::Dir {
                root: "/abs".into(),
                entry: "index.html".into(),
                interactive: true,
                forward_keys: vec![],
                preload: vec![],
            })
        );
    }

    /// Asserts that an `inline` register carries its document and an
    /// explicit `interactive: false`.
    ///
    /// Case: a program registers a status badge that must not take input.
    #[test]
    fn parses_inline_register() {
        assert_eq!(
            parse(r#"{"op":"register","kind":"inline","html":"<h1>x</h1>","interactive":false}"#),
            ClientMsg::Register(RegisterKind::Inline {
                html: "<h1>x</h1>".into(),
                interactive: false,
                forward_keys: vec![],
                preload: vec![],
            })
        );
    }

    /// Asserts that a `register` carrying an unknown `click_focus` field
    /// still parses, the field ignored.
    ///
    /// Case: an app built against an SDK that still sends
    /// `click_focus:false` registers its view with an updated orzma.
    #[test]
    fn a_register_carrying_click_focus_still_parses() {
        let msg = parse(
            r#"{"op":"register","kind":"dir","root":"/abs","entry":"index.html","click_focus":false}"#,
        );
        assert!(matches!(msg, ClientMsg::Register(RegisterKind::Dir { .. })));
    }

    /// Asserts that `unregister` and `new_instance` lines parse into their
    /// handles.
    ///
    /// Case: a program asks for a second placement of its view and later
    /// releases the view.
    #[test]
    fn parses_unregister_and_new_instance() {
        assert_eq!(
            parse(r#"{"op":"unregister","handle":"h1"}"#),
            ClientMsg::Unregister {
                handle: "h1".into()
            }
        );
        assert_eq!(
            parse(r#"{"op":"new_instance","handle":"h1"}"#),
            ClientMsg::NewInstance {
                handle: "h1".into()
            }
        );
    }

    /// Asserts that an unknown `op` fails to parse.
    ///
    /// Case: a newer SDK sends an op this orzma does not know.
    #[test]
    fn rejects_an_unknown_op() {
        assert!(serde_json::from_str::<ClientMsg>(r#"{"op":"nope"}"#).is_err());
    }

    /// Asserts that a successful and a failed `reply` both parse, with the
    /// missing field defaulted.
    ///
    /// Case: a program answers one page call with a value and another with
    /// an error.
    #[test]
    fn parses_reply_ok_and_err() {
        assert_eq!(
            parse(r#"{"op":"reply","reqId":"g7","ok":true,"value":42}"#),
            ClientMsg::Reply {
                req_id: "g7".into(),
                ok: true,
                value: json!(42),
                error: None
            }
        );
        assert_eq!(
            parse(r#"{"op":"reply","reqId":"g8","ok":false,"error":"boom"}"#),
            ClientMsg::Reply {
                req_id: "g8".into(),
                ok: false,
                value: Value::Null,
                error: Some("boom".into())
            }
        );
    }

    /// Asserts that an `emit` line parses into its handle, name, and payload.
    ///
    /// Case: a program pushes a tick to its pages.
    #[test]
    fn parses_emit() {
        assert_eq!(
            parse(r#"{"op":"emit","handle":"H","event":"tick","payload":{"n":1}}"#),
            ClientMsg::Emit {
                handle: "H".into(),
                event: "tick".into(),
                payload: json!({"n":1})
            }
        );
    }

    /// Asserts that a `focus` line addresses one instance, and that a null
    /// instance parses as the blur request.
    ///
    /// Case: an app moves keyboard focus onto one of its placements, then
    /// hands focus back to the terminal.
    #[test]
    fn parses_focus_and_blur() {
        assert_eq!(
            parse(r#"{"op":"focus","instance":"3f5a"}"#),
            ClientMsg::Focus {
                instance: Some("3f5a".into())
            }
        );
        assert_eq!(
            parse(r#"{"op":"focus","instance":null}"#),
            ClientMsg::Focus { instance: None }
        );
    }

    /// Asserts that the socket `mount` and `unmount` ops parse with their
    /// cell and size, and that a `mount` missing its size is rejected.
    ///
    /// Case: orzmd in a Windows pane mounts its view over the socket at row
    /// 2, column 3, twelve rows by forty-eight columns.
    #[test]
    fn parses_mount_and_unmount() {
        assert_eq!(
            parse(r#"{"op":"mount","instance":"3f5a","row":2,"col":3,"rows":12,"cols":48}"#),
            ClientMsg::Mount {
                instance: "3f5a".into(),
                row: 2,
                col: 3,
                rows: 12,
                cols: 48,
            }
        );
        assert_eq!(
            parse(r#"{"op":"unmount","instance":"3f5a"}"#),
            ClientMsg::Unmount {
                instance: "3f5a".into()
            }
        );
        assert!(
            serde_json::from_str::<ClientMsg>(
                r#"{"op":"mount","instance":"3f5a","row":2,"col":3}"#
            )
            .is_err()
        );
    }

    /// Asserts that a register's forward keys parse into chords.
    ///
    /// Case: a TUI browser registers Alt+H as a forward key.
    #[test]
    fn parses_register_with_forward_keys() {
        let msg = parse(
            r#"{"op":"register","kind":"inline","html":"x","forward_keys":[{"mods":["alt"],"key":"h"}]}"#,
        );
        let ClientMsg::Register(RegisterKind::Inline { forward_keys, .. }) = msg else {
            panic!("expected an inline register");
        };
        assert_eq!(
            forward_keys,
            vec![ForwardChord::new(vec!["alt".into()], "h")]
        );
    }

    /// Asserts that a `url` register defaults to an interactive view without
    /// the bridge, and parses an explicit `bridge: true`.
    ///
    /// Case: one program shows a remote page read-only, and another embeds
    /// its own web app with the bridge.
    #[test]
    fn parses_url_register_with_and_without_the_bridge() {
        assert_eq!(
            parse(r#"{"op":"register","kind":"url","url":"https://example.com"}"#),
            ClientMsg::Register(RegisterKind::Url {
                url: "https://example.com".into(),
                interactive: true,
                bridge: false,
                forward_keys: vec![],
                preload: vec![],
            })
        );
        assert_eq!(
            parse(
                r#"{"op":"register","kind":"url","url":"https://app.example.com","bridge":true}"#
            ),
            ClientMsg::Register(RegisterKind::Url {
                url: "https://app.example.com".into(),
                interactive: true,
                bridge: true,
                forward_keys: vec![],
                preload: vec![],
            })
        );
    }

    /// Asserts that the host parses the exact `url` register line the SDK
    /// writes.
    ///
    /// Case: `ratatui_orzma` registers a remote page with every field
    /// spelled out.
    #[test]
    fn host_parses_the_exact_wire_string_the_sdk_emits() {
        assert_eq!(
            parse(
                r#"{"op":"register","kind":"url","url":"https://example.com","interactive":true,"bridge":false}"#
            ),
            ClientMsg::Register(RegisterKind::Url {
                url: "https://example.com".into(),
                interactive: true,
                bridge: false,
                forward_keys: vec![],
                preload: vec![],
            })
        );
    }

    /// Asserts that each of the three reply shapes serializes to its exact
    /// wire line.
    ///
    /// Case: one connection registers a view, asks for a second placement,
    /// then sends a request the host rejects.
    #[test]
    fn serializes_the_three_reply_shapes() {
        assert_eq!(
            serde_json::to_string(&ServerMsg::registered("h1", InstanceId(1))).unwrap(),
            r#"{"ok":true,"handle":"h1","instance":"00000000000000000000000000000001"}"#
        );
        assert_eq!(
            serde_json::to_string(&ServerMsg::instanced(InstanceId(2))).unwrap(),
            r#"{"ok":true,"instance":"00000000000000000000000000000002"}"#
        );
        assert_eq!(
            serde_json::to_string(&ServerMsg::err("unknown_handle")).unwrap(),
            r#"{"ok":false,"error":"unknown_handle"}"#
        );
    }

    /// Asserts that a compositing push names the placement it is about.
    ///
    /// Case: a program holding two placements of one handle learns that the
    /// second started painting.
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

    /// Asserts that a focus change serializes to the `focus_changed` push
    /// shape.
    ///
    /// Case: the user clicks a mounted page and its program learns that the
    /// page took the keyboard.
    #[test]
    fn serializes_focus_changed() {
        let msg = PushMsg::FocusChanged {
            handle: "h1".into(),
            instance: "i1".into(),
            focused: true,
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({"op": "focus_changed", "handle": "h1", "instance": "i1", "focused": true})
        );
    }

    /// Asserts that a call push serializes with `reqId` spelled as the wire
    /// expects.
    ///
    /// Case: a page calls `save` and its program receives the call.
    #[test]
    fn serializes_a_call_push() {
        let msg = PushMsg::Call {
            handle: "H".into(),
            instance: "i1".into(),
            req_id: "0".into(),
            method: "save".into(),
            params: json!([1, 2]),
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({"op": "call", "handle": "H", "instance": "i1", "reqId": "0", "method": "save", "params": [1, 2]})
        );
    }

    /// Asserts that an event push serializes to the `event` shape without
    /// an instance.
    ///
    /// Case: a page emits `hello` and its program receives it.
    #[test]
    fn serializes_an_event_push() {
        let msg = PushMsg::Event {
            handle: "H".into(),
            event: "hello".into(),
            payload: json!({"message": "hi"}),
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({"op": "event", "handle": "H", "event": "hello", "payload": {"message": "hi"}})
        );
    }

    /// Asserts that a `navigate` line addresses one instance and carries a
    /// history action or a target URL.
    ///
    /// Case: an embedded browser UI sends Back for the placement the user
    /// is looking at, then loads a typed address into it.
    #[test]
    fn parses_navigate() {
        assert_eq!(
            parse(r#"{"op":"navigate","instance":"3f5a","action":"back"}"#),
            ClientMsg::Navigate {
                instance: "3f5a".into(),
                action: NavAction::Back,
            }
        );
        assert_eq!(
            parse(r#"{"op":"navigate","instance":"3f5a","action":{"to":"https://example.com"}}"#),
            ClientMsg::Navigate {
                instance: "3f5a".into(),
                action: NavAction::To("https://example.com".into()),
            }
        );
    }

    /// Asserts that preload scripts parse in order and default to none.
    ///
    /// Case: one program injects a setup script and another injects nothing.
    #[test]
    fn parses_preload_and_its_default() {
        let ClientMsg::Register(RegisterKind::Inline { preload, .. }) =
            parse(r#"{"op":"register","kind":"inline","html":"x","preload":["window.A=1;"]}"#)
        else {
            panic!("expected an inline register");
        };
        assert_eq!(preload, vec!["window.A=1;".to_string()]);
        let ClientMsg::Register(RegisterKind::Inline { preload, .. }) =
            parse(r#"{"op":"register","kind":"inline","html":"x"}"#)
        else {
            panic!("expected an inline register");
        };
        assert!(preload.is_empty());
    }

    /// Asserts that a `set_forward_keys` line parses into its handle and
    /// chord list.
    ///
    /// Case: a TUI browser enters its insert mode and replaces its forward
    /// keys with Esc alone.
    #[test]
    fn parses_set_forward_keys() {
        assert_eq!(
            parse(r#"{"op":"set_forward_keys","handle":"h1","keys":[{"mods":[],"key":"esc"}]}"#),
            ClientMsg::SetForwardKeys {
                handle: "h1".into(),
                keys: vec![ForwardChord::new(vec![], "esc")],
            }
        );
    }
}
