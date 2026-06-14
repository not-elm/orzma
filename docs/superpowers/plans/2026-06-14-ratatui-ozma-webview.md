# ratatui-ozma Webview SDK Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `ratatui-ozma` crate — a ratatui `StatefulWidget` that embeds an ozmux inline webview at a cell rect, plus an RPC handler API for bidirectional `window.ozmux` communication.

**Architecture:** A `Ozma` session owns one `$OZMUX_SOCK` Unix-socket connection and one background reader thread. `Webview` is a builder (content + RPC handlers); `Ozma::register` mints a handle and returns a cloneable `WebviewHandle` (`emit`, `id`). Per frame, a `Webview` widget blanks its cells, paints an optional fallback, and records its rect into `Ozma`'s frame collector; after `terminal.draw()`, `Ozma::flush` diffs placements and emits `mount-inline`/`unmount-inline` OSC at each rect via `terminal.backend_mut()`. RPC handlers run on the reader thread; the socket write-mutex is never held across a handler.

**Tech Stack:** Rust edition 2024, `ratatui 0.29` (+ its re-exported `crossterm`), `serde`/`serde_json`, `crossbeam-channel`, `thiserror`. Tokio-free (std threads). Unix-only (`std::os::unix::net::UnixStream`); compositing is macOS-only but the protocol layers are testable everywhere via a fake control server.

Spec: `docs/superpowers/specs/2026-06-14-ratatui-ozma-webview-design.md`. Raw reference client: `examples/dyn_webview_client.rs`.

---

## File Structure

```
sdk/ratatui-ozma/
  Cargo.toml                  # deps; [lints] workspace = true
  src/
    lib.rs                    # crate root: //!, module decls, re-exports, #![warn(missing_docs)]
    error.rs                  # OzmaError, RpcError
    osc.rs                    # OSC 5379 + CUP sequence builders + validation
    protocol.rs               # wire types (register/reply/emit/call) + serde
    handler.rs                # BoxedHandler type + make_handler<P,R,F> (tuple-param dispatch)
    webview.rs                # Webview builder (inline/dir/interactive/fallback/on), WebviewHandle (emit/id)
    session.rs               # Ozma: connect/register/frame/flush + reader thread + Placement/FramePlacements
    widget.rs                 # WebviewWidget StatefulWidget (blank + fallback + record placement)
  examples/
    ratatui_webview.rs        # end-to-end demo (layout -> widget -> RPC handler -> emit loop)
  tests/
    support/mod.rs            # fake NDJSON control server over a UnixListener
    integration.rs            # register -> call -> reply -> emit round-trip vs fake server
```

**Naming note:** `WebviewWidget` is the rendered widget type; `Webview` is the pre-register builder; `WebviewHandle` is the registered handle. (Spec §8 records a possible later merge of widget+handle; v1 keeps them separate.)

**Handler ergonomics decision (locks in spec §8 option 4):** v1 ships the proven path — handler params are a **tuple** deserialized from the JSON args array (`|(arg,): (String,)|` for one arg, `|(a, b): (u32, String)|` for two, `|(): ()|` for none). The bare-`|arg: T|` extractor and a `Params` wrapper are deferred. This compiles with no proc-macros and maps 1:1 to `window.ozmux.call(method, args)`.

---

## Task 1: Crate scaffolding

**Files:**
- Modify: `sdk/ratatui-ozma/Cargo.toml`
- Modify: `sdk/ratatui-ozma/src/lib.rs`

- [ ] **Step 1: Set crate dependencies**

Replace `sdk/ratatui-ozma/Cargo.toml` with:

```toml
[package]
name = "ratatui-ozma"
version.workspace = true
edition.workspace = true
license.workspace = true
readme.workspace = true
authors.workspace = true
publish.workspace = true

[dependencies]
ratatui = "0.29"
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
crossbeam-channel = "0.5"

[lints]
workspace = true
```

- [ ] **Step 2: Write the crate root with module decls**

Replace `sdk/ratatui-ozma/src/lib.rs` with:

```rust
//! ratatui widget + RPC handler for embedding an ozmux inline webview.
//!
//! Run inside an ozmux pane: [`Ozma::connect`] dials `$OZMUX_SOCK`, [`Webview`]
//! registers content (minting a handle), [`WebviewWidget`] renders it as a
//! ratatui widget, and [`Ozma::flush`] emits the mount OSC after each draw.
#![warn(missing_docs)]

mod error;
mod handler;
mod osc;
mod protocol;
mod session;
mod webview;
mod widget;

pub use error::{OzmaError, RpcError};
pub use session::Ozma;
pub use webview::{Webview, WebviewHandle};
pub use widget::WebviewWidget;
```

- [ ] **Step 3: Stub the modules so the crate compiles**

Create each module file with a `//!` line and minimal content (later tasks fill them). Create `src/error.rs`, `src/handler.rs`, `src/osc.rs`, `src/protocol.rs`, `src/session.rs`, `src/webview.rs`, `src/widget.rs` each containing only a doc line for now — but since later tasks need real types, instead create them empty-but-doc'd and accept that `lib.rs`'s `pub use` will fail until those types exist. To keep this task green, comment the `pub use` lines temporarily:

Set the bottom of `lib.rs` to:

```rust
mod error;
mod handler;
mod osc;
mod protocol;
mod session;
mod webview;
mod widget;
```

And give each module file a single doc line, e.g. `src/error.rs`:

```rust
//! Error types for the ratatui-ozma SDK.
```

Use the matching one-line `//!` for the others:
- `src/handler.rs`: `//! RPC handler boxing and tuple-param dispatch.`
- `src/osc.rs`: `//! OSC 5379 and CUP escape-sequence builders.`
- `src/protocol.rs`: `//! Control-socket NDJSON wire types.`
- `src/session.rs`: `//! The Ozma session: socket connection, reader thread, flush.`
- `src/webview.rs`: `//! Webview builder and registered handle.`
- `src/widget.rs`: `//! The ratatui StatefulWidget that records placements.`

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p ratatui-ozma`
Expected: PASS (downloads ratatui; empty modules compile).

- [ ] **Step 5: Commit**

```bash
git add sdk/ratatui-ozma/Cargo.toml sdk/ratatui-ozma/src/
git commit -m "feat(ratatui-ozma): scaffold crate with deps and module skeleton"
```

---

## Task 2: Error types

**Files:**
- Modify: `sdk/ratatui-ozma/src/error.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/error.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_rejection_renders_reason() {
        let e = OzmaError::Register {
            reason: "html_too_large".to_owned(),
        };
        assert!(e.to_string().contains("html_too_large"));
    }

    #[test]
    fn rpc_error_message_roundtrips() {
        let e = RpcError::new("unknown_method");
        assert_eq!(e.message(), "unknown_method");
    }

    #[test]
    fn io_error_converts() {
        let io = std::io::Error::other("boom");
        let e: OzmaError = io.into();
        assert!(matches!(e, OzmaError::Io(_)));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma error`
Expected: FAIL (types not defined).

- [ ] **Step 3: Write the implementation**

Put this above the `#[cfg(test)]` block in `src/error.rs` (under the `//!` line):

```rust
use std::sync::PoisonError;

/// An error from the ratatui-ozma SDK.
#[derive(Debug, thiserror::Error)]
pub enum OzmaError {
    /// `$OZMUX_SOCK` or `$OZMUX_TOKEN` was unset — not running inside an ozmux pane.
    #[error("not inside an ozmux pane: {0} is unset")]
    NotInPane(&'static str),

    /// A socket connect/read/write failure.
    #[error("control-socket io error: {0}")]
    Io(#[from] std::io::Error),

    /// The control plane rejected a `register` request.
    #[error("register rejected: {reason}")]
    Register {
        /// The control-plane error string (e.g. `html_too_large`, `unsafe_entry`).
        reason: String,
    },

    /// The connection closed while a `register` reply was pending.
    #[error("control socket closed before register reply")]
    Disconnected,

    /// A serde (de)serialization failure.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// An internal lock was poisoned by a panicked thread.
    #[error("internal lock poisoned")]
    Poisoned,
}

impl<T> From<PoisonError<T>> for OzmaError {
    fn from(_: PoisonError<T>) -> Self {
        OzmaError::Poisoned
    }
}

/// An error returned by an RPC handler, surfaced to the page as a rejected Promise.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct RpcError {
    message: String,
}

impl RpcError {
    /// Creates an `RpcError` with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the error message sent back to the page.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<serde_json::Error> for RpcError {
    fn from(e: serde_json::Error) -> Self {
        RpcError::new(e.to_string())
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ratatui-ozma error`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add sdk/ratatui-ozma/src/error.rs
git commit -m "feat(ratatui-ozma): OzmaError and RpcError types"
```

---

## Task 3: OSC + CUP sequence builders

**Files:**
- Modify: `sdk/ratatui-ozma/src/osc.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/osc.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_sequence_is_canonical() {
        let s = mount_inline("memo.main", 12, 48).unwrap();
        assert_eq!(s, "\x1b]5379;mount-inline;memo.main;12;48\x1b\\");
    }

    #[test]
    fn unmount_sequence_is_canonical() {
        assert_eq!(unmount_inline("memo.main"), "\x1b]5379;unmount-inline;memo.main\x1b\\");
    }

    #[test]
    fn cup_is_one_based() {
        assert_eq!(cursor_to(0, 0), "\x1b[1;1H");
        assert_eq!(cursor_to(4, 9), "\x1b[5;10H");
    }

    #[test]
    fn rejects_out_of_range_dims() {
        assert!(mount_inline("h", 0, 10).is_err());
        assert!(mount_inline("h", 201, 10).is_err());
        assert!(mount_inline("h", 10, 401).is_err());
    }

    #[test]
    fn rejects_bad_handle_charset() {
        assert!(mount_inline("bad handle", 10, 10).is_err());
        assert!(mount_inline("", 10, 10).is_err());
    }

    #[test]
    fn clamp_fits_range() {
        assert_eq!(clamp_dims(0, 0), (1, 1));
        assert_eq!(clamp_dims(500, 500), (200, 400));
        assert_eq!(clamp_dims(12, 48), (12, 48));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma osc`
Expected: FAIL (functions not defined).

- [ ] **Step 3: Write the implementation**

Put above the test block in `src/osc.rs`:

```rust
use crate::error::OzmaError;

/// Max inline-webview rows accepted by the VT layer (`1..=MAX_ROWS`).
pub(crate) const MAX_ROWS: u16 = 200;
/// Max inline-webview cols accepted by the VT layer (`1..=MAX_COLS`).
pub(crate) const MAX_COLS: u16 = 400;

/// Returns the `mount-inline` OSC 5379 sequence, or an error if the handle
/// charset is invalid or the dimensions are out of range.
pub(crate) fn mount_inline(handle: &str, rows: u16, cols: u16) -> Result<String, OzmaError> {
    validate_handle(handle)?;
    if !(1..=MAX_ROWS).contains(&rows) || !(1..=MAX_COLS).contains(&cols) {
        return Err(OzmaError::Register {
            reason: format!("geometry out of range: {rows}x{cols}"),
        });
    }
    Ok(format!("\x1b]5379;mount-inline;{handle};{rows};{cols}\x1b\\"))
}

/// Returns the `unmount-inline` OSC 5379 sequence for a single view handle.
pub(crate) fn unmount_inline(handle: &str) -> String {
    format!("\x1b]5379;unmount-inline;{handle}\x1b\\")
}

/// Returns a CUP (cursor position) sequence for a 0-based viewport cell.
pub(crate) fn cursor_to(row: u16, col: u16) -> String {
    format!("\x1b[{};{}H", row.saturating_add(1), col.saturating_add(1))
}

/// Clamps a (rows, cols) pair into the accepted `1..=MAX` range.
pub(crate) fn clamp_dims(rows: u16, cols: u16) -> (u16, u16) {
    (rows.clamp(1, MAX_ROWS), cols.clamp(1, MAX_COLS))
}

/// Validates a view handle against the `^[A-Za-z0-9._-]{1,128}$` charset.
fn validate_handle(handle: &str) -> Result<(), OzmaError> {
    let ok = (1..=128).contains(&handle.len())
        && handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(OzmaError::Register {
            reason: format!("invalid view handle: {handle:?}"),
        })
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ratatui-ozma osc`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add sdk/ratatui-ozma/src/osc.rs
git commit -m "feat(ratatui-ozma): OSC mount/unmount + CUP sequence builders"
```

---

## Task 4: Wire protocol types

**Files:**
- Modify: `sdk/ratatui-ozma/src/protocol.rs`

These mirror the control-plane NDJSON (`src/control_plane/protocol.rs`): outbound `hello`/`register`/`unregister`/`reply`/`emit`, inbound `call` and the untagged register reply `{ok, handle}` / `{ok:false, error}`.

- [ ] **Step 1: Write the failing test**

Append to `src/protocol.rs`:

```rust
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
        let c: IncomingCall =
            serde_json::from_str(r#"{"op":"call","handle":"h","reqId":"3","method":"ping","args":["x"]}"#)
                .unwrap();
        assert_eq!(c.handle, "h");
        assert_eq!(c.method, "ping");
        assert_eq!(c.args, vec![serde_json::json!("x")]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma protocol`
Expected: FAIL (types not defined).

- [ ] **Step 3: Write the implementation**

Put above the test block in `src/protocol.rs`:

```rust
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
    /// Tear down a previously-registered handle.
    Unregister {
        /// The handle to drop.
        handle: String,
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
    use serde::ser::SerializeMap;
    use serde::Serializer;
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ratatui-ozma protocol`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add sdk/ratatui-ozma/src/protocol.rs
git commit -m "feat(ratatui-ozma): control-socket wire protocol types"
```

---

## Task 5: RPC handler boxing + tuple dispatch

**Files:**
- Modify: `sdk/ratatui-ozma/src/handler.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/handler.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn single_arg_tuple_dispatch() {
        let h = make_handler(|(name,): (String,)| Ok(format!("hi {name}")));
        let out = h(vec![json!("ada")]).unwrap();
        assert_eq!(out, json!("hi ada"));
    }

    #[test]
    fn two_arg_tuple_dispatch() {
        let h = make_handler(|(a, b): (u32, u32)| Ok(a + b));
        assert_eq!(h(vec![json!(2), json!(3)]).unwrap(), json!(5));
    }

    #[test]
    fn unit_arg_dispatch() {
        let h = make_handler(|(): ()| Ok("ok"));
        assert_eq!(h(vec![]).unwrap(), json!("ok"));
    }

    #[test]
    fn bad_args_become_rpc_error() {
        let h = make_handler(|(_n,): (u32,)| Ok(0u32));
        assert!(h(vec![json!("not a number")]).is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma handler`
Expected: FAIL (functions not defined).

- [ ] **Step 3: Write the implementation**

Put above the test block in `src/handler.rs`:

```rust
use crate::error::RpcError;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

/// A type-erased RPC handler: args array in, JSON value or error out.
pub(crate) type BoxedHandler =
    Arc<dyn Fn(Vec<Value>) -> Result<Value, RpcError> + Send + Sync + 'static>;

/// Boxes a typed handler whose parameter is a tuple deserialized from the
/// positional args array, returning a `BoxedHandler`.
pub(crate) fn make_handler<P, R, F>(f: F) -> BoxedHandler
where
    P: DeserializeOwned,
    R: Serialize,
    F: Fn(P) -> Result<R, RpcError> + Send + Sync + 'static,
{
    Arc::new(move |args: Vec<Value>| {
        let params: P = serde_json::from_value(Value::Array(args))?;
        let result = f(params)?;
        Ok(serde_json::to_value(result)?)
    })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ratatui-ozma handler`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add sdk/ratatui-ozma/src/handler.rs
git commit -m "feat(ratatui-ozma): tuple-param RPC handler boxing"
```

---

## Task 6: Webview builder + WebviewHandle

**Files:**
- Modify: `sdk/ratatui-ozma/src/webview.rs`

`WebviewHandle::emit` writes through a shared `Arc<Mutex<UnixStream>>` (the same write half `Ozma` holds). Built in Task 8; here we define the types and the builder, testing the builder shape (emit is covered by the integration test in Task 9).

- [ ] **Step 1: Write the failing test**

Append to `src/webview.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inline_builder_records_kind_and_default_interactive() {
        let wv = Webview::inline("<h1>hi</h1>");
        match &wv.kind {
            RegisterKind::Inline { html, interactive } => {
                assert_eq!(html, "<h1>hi</h1>");
                assert!(*interactive);
            }
            _ => panic!("expected inline"),
        }
    }

    #[test]
    fn dir_builder_and_non_interactive() {
        let wv = Webview::dir("/abs/ui", "index.html").interactive(false);
        match &wv.kind {
            RegisterKind::Dir { root, entry, interactive } => {
                assert_eq!(root, "/abs/ui");
                assert_eq!(entry, "index.html");
                assert!(!*interactive);
            }
            _ => panic!("expected dir"),
        }
    }

    #[test]
    fn on_registers_handler() {
        let wv = Webview::inline("x").on("ping", |(n,): (String,)| Ok(format!("pong:{n}")));
        let h = wv.handlers.get("ping").expect("handler present");
        assert_eq!(h(vec![json!("hi")]).unwrap(), json!("pong:hi"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma webview`
Expected: FAIL (types not defined).

- [ ] **Step 3: Write the implementation**

Put above the test block in `src/webview.rs`:

```rust
use crate::error::OzmaError;
use crate::handler::{make_handler, BoxedHandler};
use crate::protocol::{ClientMsg, RegisterKind};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The shared write half of the control socket.
pub(crate) type SharedWriter = Arc<Mutex<UnixStream>>;

/// A webview definition: content plus RPC handlers, before registration.
pub struct Webview {
    pub(crate) kind: RegisterKind,
    pub(crate) handlers: HashMap<String, BoxedHandler>,
}

impl Webview {
    /// Creates a webview from a full inline HTML document.
    pub fn inline(html: impl Into<String>) -> Self {
        Self {
            kind: RegisterKind::Inline {
                html: html.into(),
                interactive: true,
            },
            handlers: HashMap::new(),
        }
    }

    /// Creates a webview served from a directory of assets.
    pub fn dir(root: impl AsRef<Path>, entry: impl Into<String>) -> Self {
        Self {
            kind: RegisterKind::Dir {
                root: root.as_ref().display().to_string(),
                entry: entry.into(),
                interactive: true,
            },
            handlers: HashMap::new(),
        }
    }

    /// Sets the control-plane `interactive` flag (focus/input). Fixed at register.
    pub fn interactive(mut self, interactive: bool) -> Self {
        match &mut self.kind {
            RegisterKind::Inline { interactive: i, .. } => *i = interactive,
            RegisterKind::Dir { interactive: i, .. } => *i = interactive,
        }
        self
    }

    /// Registers an RPC handler for `method`. The parameter is a tuple
    /// deserialized from the page's `window.ozmux.call(method, args)` array.
    pub fn on<P, R, F>(mut self, method: impl Into<String>, f: F) -> Self
    where
        P: DeserializeOwned,
        R: Serialize,
        F: Fn(P) -> Result<R, crate::error::RpcError> + Send + Sync + 'static,
    {
        self.handlers.insert(method.into(), make_handler(f));
        self
    }
}

/// A registered webview handle: emit events to the page, read its id.
#[derive(Clone)]
pub struct WebviewHandle {
    id: String,
    writer: SharedWriter,
}

impl WebviewHandle {
    pub(crate) fn new(id: String, writer: SharedWriter) -> Self {
        Self { id, writer }
    }

    /// Returns the opaque handle id minted by the control plane.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Pushes an event to the currently-mounted page(s) of this handle.
    ///
    /// Mount-scoped: a no-op (still `Ok`) when nothing is mounted.
    pub fn emit<T: Serialize>(&self, event: &str, payload: &T) -> Result<(), OzmaError> {
        let msg = ClientMsg::Emit {
            handle: self.id.clone(),
            event: event.to_owned(),
            payload: serde_json::to_value(payload)?,
        };
        let line = serde_json::to_string(&msg)?;
        let mut w = self.writer.lock()?;
        writeln!(w, "{line}")?;
        w.flush()?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ratatui-ozma webview`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add sdk/ratatui-ozma/src/webview.rs
git commit -m "feat(ratatui-ozma): Webview builder and WebviewHandle"
```

---

## Task 7: Fake control server (test support)

**Files:**
- Create: `sdk/ratatui-ozma/tests/support/mod.rs`

A `UnixListener`-backed fake speaking the NDJSON protocol, so integration tests run without a real ozmux. Lives in `tests/support/` (a subdirectory module, so Cargo does not treat it as its own test binary).

- [ ] **Step 1: Write the fake server**

Create `sdk/ratatui-ozma/tests/support/mod.rs`:

```rust
//! A fake ozmux control server for integration tests.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// A fake control server: accepts one client, replies to the first `register`
/// with a fixed handle, and forwards every client line over `received`.
pub struct FakeServer {
    pub sock_path: std::path::PathBuf,
    received: Receiver<Value>,
    server_writer: UnixStream,
    _dir: tempfile::TempDir,
}

impl FakeServer {
    /// Boots on a temp socket, waits for one client, and answers its register.
    pub fn start(handle: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("control.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        let (conn_tx, conn_rx) = mpsc::channel::<UnixStream>();
        let (recv_tx, received) = mpsc::channel::<Value>();

        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            conn_tx.send(stream.try_clone().unwrap()).unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                    let _ = recv_tx.send(v);
                }
            }
        });

        let server_writer = conn_rx.recv().unwrap();
        let me = Self {
            sock_path,
            received,
            server_writer,
            _dir: dir,
        };
        me.drain_until_register();
        let mut me = me;
        me.send(json!({ "ok": true, "handle": handle }));
        me
    }

    fn drain_until_register(&self) {
        loop {
            let v = self.received.recv().unwrap();
            if v["op"] == "register" {
                return;
            }
        }
    }

    /// Sends a raw JSON line to the connected client.
    pub fn send(&mut self, v: Value) {
        writeln!(self.server_writer, "{v}").unwrap();
        self.server_writer.flush().unwrap();
    }

    /// Blocks for the next post-registration line the client sent.
    pub fn next_message(&self) -> Value {
        self.received.recv().unwrap()
    }
}
```

- [ ] **Step 2: Verify it compiles as part of the test target later**

This module is only compiled when a test in `tests/` declares `mod support;`. No standalone check yet; Task 9 exercises it.

- [ ] **Step 3: Add `tempfile` as a dev-dependency**

Append to `sdk/ratatui-ozma/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 4: Commit**

```bash
git add sdk/ratatui-ozma/tests/support/mod.rs sdk/ratatui-ozma/Cargo.toml
git commit -m "test(ratatui-ozma): fake NDJSON control server"
```

---

## Task 8: Ozma session — connect, register, frame, flush

**Files:**
- Modify: `sdk/ratatui-ozma/src/session.rs`

This task adds `connect`/`register` (and the reader thread skeleton that fulfills register replies + dispatches calls), plus `Placement`/`FramePlacements`/`frame`/`flush`. RPC dispatch correctness is verified in Task 9; here the unit test covers `flush` diffing against an in-memory writer.

- [ ] **Step 1: Write the failing test (flush diffing)**

Append to `src/session.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect { x, y, width: w, height: h }
    }

    #[test]
    fn flush_emits_mount_then_skips_unchanged() {
        let mut state = FlushState::default();
        let mut placements = vec![Placement { handle: "h1".into(), area: rect(2, 3, 48, 12) }];

        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &placements).unwrap();
        let first = String::from_utf8(buf).unwrap();
        assert!(first.contains("\x1b[4;3H"));
        assert!(first.contains("mount-inline;h1;12;48"));

        let mut buf2 = Vec::new();
        flush_placements(&mut buf2, &mut state, &placements).unwrap();
        assert!(String::from_utf8(buf2).unwrap().is_empty(), "unchanged frame emits nothing");

        placements[0].area = rect(2, 3, 50, 12);
        let mut buf3 = Vec::new();
        flush_placements(&mut buf3, &mut state, &placements).unwrap();
        assert!(String::from_utf8(buf3).unwrap().contains("mount-inline;h1;12;50"));
    }

    #[test]
    fn flush_unmounts_vanished_handle() {
        let mut state = FlushState::default();
        let placements = vec![Placement { handle: "h1".into(), area: rect(0, 0, 10, 5) }];
        flush_placements(&mut Vec::new(), &mut state, &placements).unwrap();

        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &[]).unwrap();
        assert!(String::from_utf8(buf).unwrap().contains("unmount-inline;h1"));
    }

    #[test]
    fn flush_skips_degenerate_area() {
        let mut state = FlushState::default();
        let placements = vec![Placement { handle: "h1".into(), area: rect(0, 0, 0, 5) }];
        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &placements).unwrap();
        assert!(String::from_utf8(buf).unwrap().is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma session`
Expected: FAIL (types/functions not defined).

- [ ] **Step 3: Write the implementation**

Put above the test block in `src/session.rs`:

```rust
use crate::error::OzmaError;
use crate::handler::BoxedHandler;
use crate::osc::{clamp_dims, cursor_to, mount_inline, unmount_inline};
use crate::protocol::{ClientMsg, IncomingCall, RegisterReply};
use crate::webview::{SharedWriter, Webview, WebviewHandle};
use crossbeam_channel::{bounded, Receiver, Sender};
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::thread;

/// One webview's requested position this frame.
#[derive(Debug, Clone)]
pub(crate) struct Placement {
    pub(crate) handle: String,
    pub(crate) area: Rect,
}

/// The per-frame collector handed to the [`crate::WebviewWidget`] as its state.
#[derive(Debug, Default)]
pub struct FramePlacements {
    placements: Vec<Placement>,
}

impl FramePlacements {
    pub(crate) fn record(&mut self, handle: String, area: Rect) {
        self.placements.push(Placement { handle, area });
    }
}

/// Last-emitted geometry per handle, for diff-driven flush.
#[derive(Debug, Default)]
pub(crate) struct FlushState {
    last: HashMap<String, (u16, u16, u16, u16)>,
}

/// Shared map from handle to its method handlers, read by the reader thread.
type HandlerRegistry = Arc<Mutex<HashMap<String, Arc<HashMap<String, BoxedHandler>>>>>;
/// FIFO of pending register-reply senders (register replies are untagged).
type PendingRegisters = Arc<Mutex<VecDeque<Sender<Result<String, OzmaError>>>>>;

/// An ozmux session: owns the control-socket connection and reader thread.
pub struct Ozma {
    writer: SharedWriter,
    handlers: HandlerRegistry,
    pending: PendingRegisters,
    frame: FramePlacements,
    flush_state: FlushState,
}

impl Ozma {
    /// Connects to `$OZMUX_SOCK`, performs the `hello` handshake, and spawns the
    /// background reader thread.
    pub fn connect() -> Result<Self, OzmaError> {
        let sock = std::env::var("OZMUX_SOCK").map_err(|_| OzmaError::NotInPane("OZMUX_SOCK"))?;
        let token =
            std::env::var("OZMUX_TOKEN").map_err(|_| OzmaError::NotInPane("OZMUX_TOKEN"))?;
        let stream = UnixStream::connect(sock)?;
        let writer: SharedWriter = Arc::new(Mutex::new(stream.try_clone()?));
        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let pending: PendingRegisters = Arc::new(Mutex::new(VecDeque::new()));

        {
            let line = serde_json::to_string(&ClientMsg::Hello { token })?;
            let mut w = writer.lock()?;
            writeln!(w, "{line}")?;
            w.flush()?;
        }

        spawn_reader(stream, writer.clone(), handlers.clone(), pending.clone());

        Ok(Self {
            writer,
            handlers,
            pending,
            frame: FramePlacements::default(),
            flush_state: FlushState::default(),
        })
    }

    /// Registers a webview, blocking until the control plane mints its handle.
    pub fn register(&self, webview: Webview) -> Result<WebviewHandle, OzmaError> {
        let (tx, rx) = bounded(1);
        self.pending.lock()?.push_back(tx);

        let line = serde_json::to_string(&ClientMsg::Register(webview.kind))?;
        {
            let mut w = self.writer.lock()?;
            writeln!(w, "{line}")?;
            w.flush()?;
        }

        let handle = rx.recv().map_err(|_| OzmaError::Disconnected)??;
        self.handlers
            .lock()?
            .insert(handle.clone(), Arc::new(webview.handlers));
        Ok(WebviewHandle::new(handle, self.writer.clone()))
    }

    /// Returns the per-frame placement collector, cleared, for `render_stateful_widget`.
    pub fn frame(&mut self) -> &mut FramePlacements {
        self.frame.placements.clear();
        &mut self.frame
    }

    /// Emits mount/unmount OSC for this frame's placements, after `terminal.draw()`.
    pub fn flush<B: Backend + Write>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<(), OzmaError> {
        let placements = std::mem::take(&mut self.frame.placements);
        flush_placements(terminal.backend_mut(), &mut self.flush_state, &placements)?;
        self.frame.placements = placements;
        Ok(())
    }
}

/// Emits CUP + mount-inline for new/changed placements and unmount for vanished
/// handles, updating `state` to the new frame. Degenerate rects are skipped.
pub(crate) fn flush_placements(
    out: &mut impl Write,
    state: &mut FlushState,
    placements: &[Placement],
) -> Result<(), OzmaError> {
    let mut current: HashMap<String, (u16, u16, u16, u16)> = HashMap::new();
    for p in placements {
        if p.area.width == 0 || p.area.height == 0 {
            continue;
        }
        let (rows, cols) = clamp_dims(p.area.height, p.area.width);
        let key = (p.area.y, p.area.x, rows, cols);
        current.insert(p.handle.clone(), key);
        if state.last.get(&p.handle) != Some(&key) {
            let seq = mount_inline(&p.handle, rows, cols)?;
            write!(out, "{}{}", cursor_to(p.area.y, p.area.x), seq)?;
        }
    }
    for handle in state.last.keys() {
        if !current.contains_key(handle) {
            write!(out, "{}", unmount_inline(handle))?;
        }
    }
    out.flush()?;
    state.last = current;
    Ok(())
}

fn spawn_reader(
    stream: UnixStream,
    writer: SharedWriter,
    handlers: HandlerRegistry,
    pending: PendingRegisters,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let is_call = serde_json::from_str::<serde_json::Value>(trimmed)
                .ok()
                .map(|v| v["op"] == "call")
                .unwrap_or(false);
            if is_call {
                if let Ok(call) = serde_json::from_str::<IncomingCall>(trimmed) {
                    dispatch_call(&writer, &handlers, call);
                }
            } else if let Ok(reply) = serde_json::from_str::<RegisterReply>(trimmed) {
                if let Some(tx) = pending.lock().ok().and_then(|mut q| q.pop_front()) {
                    let outcome = if reply.ok {
                        reply
                            .handle
                            .ok_or_else(|| OzmaError::Register { reason: "missing handle".into() })
                    } else {
                        Err(OzmaError::Register {
                            reason: reply.error.unwrap_or_else(|| "unknown".into()),
                        })
                    };
                    let _ = tx.send(outcome);
                }
            }
        }
    });
}

fn dispatch_call(writer: &SharedWriter, handlers: &HandlerRegistry, call: IncomingCall) {
    let handler = handlers
        .lock()
        .ok()
        .and_then(|map| map.get(&call.handle).cloned())
        .and_then(|methods| methods.get(&call.method).cloned());

    let result = match handler {
        Some(h) => h(call.args).map_err(|e| e.message().to_owned()),
        None => Err("unknown_method".to_owned()),
    };

    let msg = ClientMsg::Reply {
        req_id: call.req_id,
        result: result.map_err(|e| e),
    };
    if let Ok(line) = serde_json::to_string(&msg) {
        if let Ok(mut w) = writer.lock() {
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
    }
}
```

> The `result.map_err(|e| e)` is intentional: `result` is already `Result<Value, String>`; the line keeps the `Reply` construction explicit. Remove the redundant `.map_err` if clippy flags it.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ratatui-ozma session`
Expected: PASS (3 tests).

- [ ] **Step 5: Wire the re-export**

Add to `src/lib.rs` re-exports (it already lists `pub use session::Ozma;`). Confirm `cargo build -p ratatui-ozma` passes.

- [ ] **Step 6: Commit**

```bash
git add sdk/ratatui-ozma/src/session.rs sdk/ratatui-ozma/src/lib.rs
git commit -m "feat(ratatui-ozma): Ozma session, reader thread, diff-driven flush"
```

---

## Task 9: End-to-end RPC integration test

**Files:**
- Create: `sdk/ratatui-ozma/tests/integration.rs`

Drives the full path against the fake server: connect → register → server sends a `call` → SDK handler replies → server sends nothing → `emit` reaches the server.

- [ ] **Step 1: Write the integration test**

Create `sdk/ratatui-ozma/tests/integration.rs`:

```rust
mod support;

use ratatui_ozma::{Ozma, Webview};
use serde_json::json;
use support::FakeServer;

fn with_env(sock: &std::path::Path, f: impl FnOnce()) {
    // SAFETY: tests in this binary run serially within this fn; env is set/unset around f.
    unsafe {
        std::env::set_var("OZMUX_SOCK", sock);
        std::env::set_var("OZMUX_TOKEN", "test-token");
    }
    f();
    unsafe {
        std::env::remove_var("OZMUX_SOCK");
        std::env::remove_var("OZMUX_TOKEN");
    }
}

#[test]
fn call_is_dispatched_and_replied() {
    let mut server = FakeServer::start("view-1");
    with_env(&server.sock_path.clone(), || {
        let ozma = Ozma::connect().unwrap();
        let _handle = ozma
            .register(Webview::inline("<h1>x</h1>").on("ping", |(n,): (String,)| {
                Ok(format!("pong:{n}"))
            }))
            .unwrap();

        server.send(json!({
            "op": "call", "handle": "view-1", "reqId": "7", "method": "ping", "args": ["hi"]
        }));

        let reply = server.next_message();
        assert_eq!(reply["op"], "reply");
        assert_eq!(reply["reqId"], "7");
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["value"], "pong:hi");
    });
}

#[test]
fn unknown_method_replies_error() {
    let mut server = FakeServer::start("view-2");
    with_env(&server.sock_path.clone(), || {
        let ozma = Ozma::connect().unwrap();
        let _h = ozma.register(Webview::inline("x")).unwrap();
        server.send(json!({
            "op": "call", "handle": "view-2", "reqId": "1", "method": "nope", "args": []
        }));
        let reply = server.next_message();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"], "unknown_method");
    });
}

#[test]
fn emit_reaches_the_server() {
    let mut server = FakeServer::start("view-3");
    with_env(&server.sock_path.clone(), || {
        let ozma = Ozma::connect().unwrap();
        let handle = ozma.register(Webview::inline("x")).unwrap();
        handle.emit("tick", &42u32).unwrap();
        let msg = server.next_message();
        assert_eq!(msg["op"], "emit");
        assert_eq!(msg["handle"], "view-3");
        assert_eq!(msg["event"], "tick");
        assert_eq!(msg["payload"], 42);
    });
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test -p ratatui-ozma --test integration`
Expected: PASS (3 tests). If a test hangs, the reader thread or fake server blocked — verify `drain_until_register` consumed both `hello` and `register` before the test sends a `call`.

- [ ] **Step 3: Commit**

```bash
git add sdk/ratatui-ozma/tests/integration.rs
git commit -m "test(ratatui-ozma): end-to-end register/call/reply/emit"
```

---

## Task 10: WebviewWidget (StatefulWidget)

**Files:**
- Modify: `sdk/ratatui-ozma/src/widget.rs`

Blanks its cells (so the webview shows through), paints an optional fallback under-layer, and records its rect into `FramePlacements`. Generic over the fallback widget; the default is a private `Blank` (renders nothing).

- [ ] **Step 1: Write the failing test**

Append to `src/widget.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::FramePlacements;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::text::Text;
    use ratatui::widgets::{StatefulWidget, Widget};

    #[test]
    fn records_placement_and_blanks_cells() {
        let area = Rect { x: 1, y: 1, width: 6, height: 2 };
        let mut buf = Buffer::filled(Rect::new(0, 0, 10, 5), ratatui::buffer::Cell::new("Z"));
        let mut state = FramePlacements::default();

        WebviewWidget::new("view-x").render(area, &mut buf, &mut state);

        assert_eq!(state.placements_for_test().len(), 1);
        assert_eq!(state.placements_for_test()[0].handle, "view-x");
        assert_eq!(buf[(1, 1)].symbol(), " ");
    }

    #[test]
    fn fallback_is_painted() {
        let area = Rect { x: 0, y: 0, width: 5, height: 1 };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();

        WebviewWidget::new("v").fallback(Text::raw("hi")).render(area, &mut buf, &mut state);

        assert_eq!(buf[(0, 0)].symbol(), "h");
    }
}
```

- [ ] **Step 2: Add a test accessor on FramePlacements**

In `src/session.rs`, inside `impl FramePlacements`, add:

```rust
    #[cfg(test)]
    pub(crate) fn placements_for_test(&self) -> &[Placement] {
        &self.placements
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma widget`
Expected: FAIL (`WebviewWidget` not defined).

- [ ] **Step 4: Write the implementation**

Put above the test block in `src/widget.rs`:

```rust
use crate::session::FramePlacements;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Clear, StatefulWidget, Widget};

/// A ratatui widget that mounts an ozmux webview at its area.
///
/// Blanks its cells (the webview composites under the text) and records its rect
/// for the next [`crate::Ozma::flush`]. Optionally paints a fallback under-layer
/// (shown on non-macOS or before the page composites).
pub struct WebviewWidget<'a, W = Blank> {
    handle: &'a str,
    fallback: W,
}

impl<'a> WebviewWidget<'a, Blank> {
    /// Creates a widget for the given webview handle id.
    pub fn new(handle: &'a str) -> Self {
        Self { handle, fallback: Blank }
    }
}

impl<'a, W> WebviewWidget<'a, W> {
    /// Sets a fallback widget painted into the cells under the webview.
    pub fn fallback<W2: Widget>(self, widget: W2) -> WebviewWidget<'a, W2> {
        WebviewWidget {
            handle: self.handle,
            fallback: widget,
        }
    }
}

impl<W: Widget> StatefulWidget for WebviewWidget<'_, W> {
    type State = FramePlacements;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        Clear.render(area, buf);
        self.fallback.render(area, buf);
        state.record(self.handle.to_owned(), area);
    }
}

/// A no-op fallback widget (the default): renders nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct Blank;

impl Widget for Blank {
    fn render(self, _area: Rect, _buf: &mut Buffer) {}
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p ratatui-ozma widget`
Expected: PASS (2 tests). If `WebviewWidget::new` does not need `mut`, that is fine.

- [ ] **Step 6: Export `Blank`**

In `src/lib.rs`, change `pub use widget::WebviewWidget;` to:

```rust
pub use widget::{Blank, WebviewWidget};
```

- [ ] **Step 7: Commit**

```bash
git add sdk/ratatui-ozma/src/widget.rs sdk/ratatui-ozma/src/session.rs sdk/ratatui-ozma/src/lib.rs
git commit -m "feat(ratatui-ozma): WebviewWidget StatefulWidget with fallback"
```

---

## Task 11: End-to-end example

**Files:**
- Create: `sdk/ratatui-ozma/examples/ratatui_webview.rs`

The ergonomic twin of `examples/dyn_webview_client.rs`: enter the alt screen, register a page with a `ping` handler, render it as a widget each frame, emit a `tick` every second, quit on `q`.

- [ ] **Step 1: Write the example**

Create `sdk/ratatui-ozma/examples/ratatui_webview.rs`:

```rust
//! Run inside an ozmux pane: `cargo run -p ratatui-ozma --example ratatui_webview`.
//!
//! Renders a webview widget in the alternate screen, replies to `ping`, and
//! emits a `tick` event every second. Press `q` to quit.
use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui_ozma::{Ozma, Webview, WebviewWidget};
use std::io::stdout;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ozma = Ozma::connect()?;
    let html = concat!(
        "<body style='background:#13131a;color:#8be9fd;font:16px sans-serif;margin:0;padding:8px'>",
        "<h1>ratatui-ozma</h1><div id='out'>calling ping…</div><div id='tick'>no ticks</div>",
        "<script>",
        "window.ozmux.call('ping',['hi']).then(v=>out.textContent='ping → '+v);",
        "window.ozmux.on('tick',n=>tick.textContent='tick #'+n);",
        "</script></body>"
    );
    let view = ozma.register(
        Webview::inline(html).on("ping", |(arg,): (String,)| Ok::<_, ratatui_ozma::RpcError>(format!("pong:{arg}"))),
    )?;

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut n: u64 = 0;
    let mut last = Instant::now();
    let result = run(&mut terminal, &mut ozma, &view, &mut n, &mut last);

    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ozma: &mut Ozma,
    view: &ratatui_ozma::WebviewHandle,
    n: &mut u64,
    last: &mut Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|f| {
            let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(f.area());
            f.render_widget(Paragraph::new("press q to quit"), rows[0]);
            let cols = Layout::horizontal([Constraint::Percentage(60), Constraint::Min(0)]).split(rows[1]);
            f.render_stateful_widget(
                WebviewWidget::new(view.id()).fallback(Block::bordered().title("loading…")),
                cols[0],
                ozma.frame(),
            );
        })?;
        ozma.flush(terminal)?;

        if last.elapsed() >= Duration::from_secs(1) {
            *n += 1;
            let _ = view.emit("tick", n);
            *last = Instant::now();
        }
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(k) = event::read()? {
                if k.code == KeyCode::Char('q') {
                    return Ok(());
                }
            }
        }
    }
}
```

- [ ] **Step 2: Verify the example compiles**

Run: `cargo build -p ratatui-ozma --example ratatui_webview`
Expected: PASS. (Running it requires a macOS ozmux pane; compilation is the gate here.)

- [ ] **Step 3: Commit**

```bash
git add sdk/ratatui-ozma/examples/ratatui_webview.rs
git commit -m "docs(ratatui-ozma): end-to-end ratatui webview example"
```

---

## Task 12: Final lint, format, and full test sweep

**Files:**
- Possibly modify any `src/*.rs` for clippy fixes.

- [ ] **Step 1: Run clippy with the workspace lints**

Run: `cargo clippy -p ratatui-ozma --all-targets -- -D warnings`
Expected: PASS. Fix any findings (e.g. remove the redundant `.map_err(|e| e)` noted in Task 8; drop unused imports). Re-run until clean.

- [ ] **Step 2: Format**

Run: `cargo fmt -p ratatui-ozma`
Expected: no diff after a second run.

- [ ] **Step 3: Full test sweep**

Run: `cargo test -p ratatui-ozma`
Expected: PASS — all unit tests (error, osc, protocol, handler, webview, session, widget) and the integration suite green.

- [ ] **Step 4: Verify the whole workspace still builds**

Run: `cargo build`
Expected: PASS (the new crate is a workspace member; nothing else depends on it, so this only confirms no breakage).

- [ ] **Step 5: Commit any lint/format fixes**

```bash
git add sdk/ratatui-ozma/
git commit -m "chore(ratatui-ozma): clippy + rustfmt pass"
```

---

## Self-Review Notes

- **Spec coverage:** §1 scope → Task 1; §3 object model → Tasks 6 (`Webview`/`WebviewHandle`), 8 (`Ozma`); §4 render flow/flush diff/area validation/cursor caveat → Tasks 8, 10; §5 RPC handlers/threading/emit mount-scope → Tasks 5, 6, 8, 9; §6 lifecycle/errors → Tasks 2, 8 (`Disconnected`, unmount-on-vanish); §7 testing (fake server, sequence builders, widget render, example) → Tasks 3, 7, 9, 10, 11; §8 resolutions (crossbeam-channel, `flush_to` core via `flush_placements`, method-before-deserialize, tuple handler) → Tasks 5, 8.
- **Deferred (spec §8 plan-time options, intentionally not built):** bare-`|arg: T|` extractor + `Params` wrapper (tuples used instead); widget+handle merge and `ozma.draw()` wrapper (kept separate). These do not affect correctness.
- **Type consistency:** `FramePlacements`/`Placement`/`flush_placements`/`FlushState` are defined in Task 8 and reused verbatim in Tasks 9–10; `BoxedHandler`/`make_handler` from Task 5 are used in Task 6; `SharedWriter` from Task 6 is used in Task 8; `RegisterKind`/`ClientMsg`/`IncomingCall`/`RegisterReply` from Task 4 flow through Tasks 6 and 8.
- **Known cleanups left to Task 12:** the redundant `.map_err(|e| e)` (Task 8) and any unused-import warnings are resolved in the clippy step rather than pre-emptively, so each TDD task stays focused.
