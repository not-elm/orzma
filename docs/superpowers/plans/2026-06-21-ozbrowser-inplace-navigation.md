# ozbrowser in-place navigation (CEF-native history) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Navigate ozbrowser's existing CEF webview in place (no handle re-registration) so H/L/Navigate/Reload stop causing an unmount/remount flicker, delegating session history to CEF.

**Architecture:** Add one control-socket op `ClientMsg::Navigate { handle, action }` (peer of `Emit`/`Focus`). The host apply system resolves the sending connection's mounted webview entity and either mutates its `WebviewSource` (for `To{url}`, driving bevy_cef's in-place navigation) or triggers a bevy_cef `RequestGoBack`/`RequestGoForward`/`RequestReload` event. ozbrowser registers its webview once and maps its navigation commands to `WebviewHandle::{navigate,go_back,go_forward,reload}`, dropping its own history stack.

**Tech Stack:** Rust (serde, bevy 0.18, bevy_cef path dep), ratatui, the ozmux control socket (NDJSON `ClientMsg`).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-06-21-ozbrowser-inplace-navigation-design.md`. Rust edition 2024, toolchain 1.95. Comments English; only `// TODO:`/`// NOTE:`/`// SAFETY:`. All `use` at top, one contiguous block, no inline fully-qualified paths. `pub`/`pub(crate)` items get `///` docs; private items declared last; mutable params first.
- `Navigate` rides the **control socket** (`ClientMsg`), NOT the OSC mount stream. It is **mount-scoped** (no-op when the handle has no mounted view), like `Emit`.
- Ownership gate is **`connection_id` ownership of the handle plus `owner_surface`** (the scoping `SetFocus` uses) — NOT `is_bridged()`.
- `To{url}` is validated with the existing `validate_url_source` (`http(s)`, non-empty host) and applies only to a `DynSource::Url` registration; `Dir`/`Inline` handles ignore it. v1 does NOT sync the navigated URL into `DynamicRegistry`.
- The host writes via `ConnectionWriters`; SDK writes via the shared `UnixStream` writer `emit` uses. The SDK `ClientMsg` is `#[serde(tag="op", rename_all="lowercase")]`; the host `ClientMsg` is `#[serde(tag="op", rename_all="snake_case")]` — single-word tags agree across both.
- Wire shape for `NavAction`: externally tagged enum with a newtype `To(String)`. `Back`/`Forward`/`Reload` serialize to the bare strings `"back"`/`"forward"`/`"reload"`; `To(url)` serializes to `{"to":"<url>"}`.
- Test commands: `cargo test -p ratatui-ozma` (SDK), `cargo test -p ozmux-gui` (host), `cargo test -p ozbrowser` (app). `cargo fmt` before each commit.

---

### Task 1: SDK — `Navigate` op + `WebviewHandle` navigation methods

**Files:**
- Modify: `sdk/ratatui-ozma/src/protocol.rs`
- Modify: `sdk/ratatui-ozma/src/webview.rs`
- Test: both files' `#[cfg(test)] mod tests`

**Interfaces:**
- Produces (consumed by Task 4 & mirrored by Task 2):
  - `pub(crate) enum NavAction { Back, Forward, Reload, To(String) }` in `protocol.rs`.
  - `ClientMsg::Navigate { handle: String, action: NavAction }` on the SDK `ClientMsg`.
  - `WebviewHandle::{navigate(url), go_back(), go_forward(), reload()} -> OzmaResult<()>`.

- [ ] **Step 1: Write the failing protocol tests**

In `sdk/ratatui-ozma/src/protocol.rs`'s `mod tests`, add:

```rust
    #[test]
    fn navigate_back_serializes() {
        let v = serde_json::to_value(ClientMsg::Navigate {
            handle: "H".into(),
            action: NavAction::Back,
        })
        .unwrap();
        assert_eq!(v["op"], "navigate");
        assert_eq!(v["handle"], "H");
        assert_eq!(v["action"], "back");
    }

    #[test]
    fn navigate_to_serializes_url_under_to() {
        let v = serde_json::to_value(ClientMsg::Navigate {
            handle: "H".into(),
            action: NavAction::To("https://example.com/x".into()),
        })
        .unwrap();
        assert_eq!(v["op"], "navigate");
        assert_eq!(v["action"]["to"], "https://example.com/x");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ratatui-ozma protocol::tests::navigate`
Expected: FAIL to compile — `no variant or associated item named Navigate`, `cannot find ... NavAction`.

- [ ] **Step 3: Add `NavAction` and the `Navigate` variant**

In `sdk/ratatui-ozma/src/protocol.rs`, add the enum above `RegisterKind` (after `ClientMsg`):

```rust
/// A navigation action on an already-registered handle's mounted webview.
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
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
```

Add the variant to `ClientMsg` (after `Focus`):

```rust
    /// Navigate a handle's mounted webview in place (no re-registration).
    Navigate {
        /// The target handle.
        handle: String,
        /// What to do.
        action: NavAction,
    },
```

- [ ] **Step 4: Run protocol tests to verify they pass**

Run: `cargo test -p ratatui-ozma protocol::tests::navigate`
Expected: PASS.

- [ ] **Step 5: Add the `WebviewHandle` methods**

In `sdk/ratatui-ozma/src/webview.rs`, import `NavAction` by extending the existing `use crate::protocol::...` line to include `NavAction`:

```rust
use crate::protocol::{ClientMsg, RegisterKind, NavAction};
```

(Keep the rest of that `use` as-is; merge `NavAction` into the existing braces.)

Add these methods to the `impl WebviewHandle` block, right after `emit` (they are `pub`, so they precede the `pub(crate) fn new_shared` per item-ordering):

```rust
    /// Navigates this handle's mounted webview to `url` in place (no
    /// re-registration). Mount-scoped: a no-op (still `Ok`) when nothing is
    /// mounted.
    pub fn navigate(&self, url: impl Into<String>) -> OzmaResult<()> {
        self.send_nav(NavAction::To(url.into()))
    }

    /// Goes back in the webview's native session history. Mount-scoped.
    pub fn go_back(&self) -> OzmaResult<()> {
        self.send_nav(NavAction::Back)
    }

    /// Goes forward in the webview's native session history. Mount-scoped.
    pub fn go_forward(&self) -> OzmaResult<()> {
        self.send_nav(NavAction::Forward)
    }

    /// Reloads the current page. Mount-scoped.
    pub fn reload(&self) -> OzmaResult<()> {
        self.send_nav(NavAction::Reload)
    }
```

Add the private helper at the bottom of the `impl` block (after `new_shared`, private items last):

```rust
    fn send_nav(&self, action: NavAction) -> OzmaResult<()> {
        let msg = ClientMsg::Navigate {
            handle: self.id(),
            action,
        };
        let line = serde_json::to_string(&msg)?;
        let mut w = self.writer.lock()?;
        writeln!(w, "{line}")?;
        w.flush()?;
        Ok(())
    }
```

- [ ] **Step 6: Build + run the SDK suite**

Run: `cargo test -p ratatui-ozma`
Expected: PASS (new protocol tests + all existing). `WebviewHandle` methods compile (thin wrappers; the wire is covered by the protocol tests, matching how `emit` is covered).

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add sdk/ratatui-ozma/src/protocol.rs sdk/ratatui-ozma/src/webview.rs
git commit -m "feat(ratatui-ozma): Navigate op + WebviewHandle navigate/go_back/go_forward/reload"
```

---

### Task 2: Host — `Navigate` wire type + `ControlEvent` + listener mapping

**Files:**
- Modify: `src/control_plane/protocol.rs`
- Modify: `src/control_plane/listener.rs`
- Test: both files' `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes (wire-compatible with Task 1): the `{op:"navigate", handle, action}` line.
- Produces (consumed by Task 3):
  - `pub(crate) enum NavAction { Back, Forward, Reload, To(String) }` in `src/control_plane/protocol.rs` (Deserialize).
  - `ClientMsg::Navigate { handle: String, action: NavAction }` on the host `ClientMsg`.
  - `ControlEvent::Navigate { connection_id: u64, owner_surface: Entity, handle: String, action: NavAction }`.

- [ ] **Step 1: Write the failing parse test**

In `src/control_plane/protocol.rs`'s `mod tests`, add:

```rust
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ozmux-gui control_plane::protocol::tests::parses_navigate`
Expected: FAIL to compile — `Navigate`/`NavAction` undefined.

- [ ] **Step 3: Add `NavAction` + `Navigate` to the host `ClientMsg`**

In `src/control_plane/protocol.rs`, add above `HostKeyChord`:

```rust
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
```

Add the variant to `ClientMsg` (after `Focus`):

```rust
    /// Navigate a handle's mounted webview in place.
    Navigate {
        /// The target handle.
        handle: String,
        /// What to do.
        action: NavAction,
    },
```

- [ ] **Step 4: Run the parse tests to verify they pass**

Run: `cargo test -p ozmux-gui control_plane::protocol::tests::parses_navigate`
Expected: PASS.

- [ ] **Step 5: Write the failing listener test**

In `src/control_plane/listener.rs`'s `mod tests`, add (mirroring the existing emit/focus dispatch tests — use the same `handle_client_msg` call shape the other tests use; the surrounding tests show the `events`/`out_tx` channel setup):

```rust
    #[test]
    fn navigate_msg_emits_navigate_event() {
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let (out_tx, _out_rx) = unbounded::<String>();
        let surface = Entity::from_bits(1);

        let flow = handle_client_msg(
            ClientMsg::Navigate {
                handle: "H".into(),
                action: NavAction::Reload,
            },
            7,
            surface,
            &ev_tx,
            &out_tx,
        );

        assert!(matches!(flow, ControlFlow::Continue(())));
        match ev_rx.try_recv().expect("a navigate event") {
            ControlEvent::Navigate {
                connection_id,
                owner_surface,
                handle,
                action,
            } => {
                assert_eq!(connection_id, 7);
                assert_eq!(owner_surface, surface);
                assert_eq!(handle, "H");
                assert_eq!(action, NavAction::Reload);
            }
            _ => panic!("expected Navigate"),
        }
    }
```

(`ControlEvent` carries a `Sender<ServerMsg>` and so derives no `Debug` — hence the `_ => panic!` rather than `{other:?}`. `NavAction`, `ClientMsg`, `ControlEvent`, `Entity`, `unbounded`, and `ControlFlow` are all in scope in the test module via its `use super::*;`.)

- [ ] **Step 6: Run to verify it fails**

Run: `cargo test -p ozmux-gui control_plane::listener::tests::navigate_msg_emits_navigate_event`
Expected: FAIL — `no variant ControlEvent::Navigate` / `ClientMsg::Navigate` not handled in `handle_client_msg`.

- [ ] **Step 7: Add `ControlEvent::Navigate` and the dispatch arm**

In `src/control_plane/listener.rs`, add the variant to `ControlEvent` (after `SetFocus`):

```rust
    /// An app-initiated in-place navigation of a handle's mounted webview.
    Navigate {
        /// Connection id (ownership check in apply).
        connection_id: u64,
        /// The surface the connection's token resolved to.
        owner_surface: Entity,
        /// The target handle.
        handle: String,
        /// What to do.
        action: NavAction,
    },
```

Import `NavAction` by extending `listener.rs`'s existing import (line 12) from
`use crate::control_plane::protocol::{ClientMsg, RegisterKind, ServerMsg};` to
`use crate::control_plane::protocol::{ClientMsg, NavAction, RegisterKind, ServerMsg};`.

Add the arm to `handle_client_msg`'s `match msg` (after the `Focus` arm):

```rust
        ClientMsg::Navigate { handle, action } => {
            let _ = events.send(ControlEvent::Navigate {
                connection_id,
                owner_surface,
                handle,
                action,
            });
        }
```

- [ ] **Step 8: Run the listener test to verify it passes**

Run: `cargo test -p ozmux-gui control_plane::listener`
Expected: PASS (new test + existing listener tests).

- [ ] **Step 9: Commit**

```bash
cargo fmt
git add src/control_plane/protocol.rs src/control_plane/listener.rs
git commit -m "feat(control-plane): Navigate ClientMsg + ControlEvent + listener dispatch"
```

---

### Task 3: Host — apply `Navigate` (in-place navigation)

**Files:**
- Modify: `src/control_plane.rs` (`apply_control_events` + imports)
- Test: `src/control_plane.rs`'s `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes (Task 2): `ControlEvent::Navigate { connection_id, owner_surface, handle, action }`, `NavAction`.
- Produces: the host effect — for a mounted, owned handle, `To{url}` (validated, `DynSource::Url` only) sets the child's `WebviewSource = WebviewSource::Url(url)`; `Back`/`Forward`/`Reload` `commands.trigger` the matching bevy_cef event on the child entity. Unowned/unmounted/invalid → no-op.

- [ ] **Step 1: Write the failing apply tests**

In `src/control_plane.rs`'s `mod apply_tests` (the module that already holds `apply_emit_is_dropped_for_a_non_bridged_url_view`; it `use super::*;` so `DynSource`, `DynamicView`, `DynamicRegistry`, `InlineWebview`, `OzmuxRpc`, `ControlEvents`, `WebviewAssetRegistryRes`, `WebviewAssetRegistry`, `ChildOf`, `App`, `unbounded`, `apply_control_events` are all in scope), add (model the setup on that emit test — here the child also needs `WebviewSource` and `ChildOf(owner_surface)` because Navigate is owner-scoped):

```rust
    #[test]
    fn apply_navigate_to_updates_webview_source_for_owned_url_view() {
        use bevy_cef::prelude::WebviewSource;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let surface = app.world_mut().spawn_empty().id();
        let mut reg = DynamicRegistry::default();
        reg.insert(
            "H".into(),
            DynamicView {
                source: DynSource::Url {
                    url: "https://example.com".into(),
                    bridge: true,
                },
                entry: String::new(),
                interactive: true,
                owner_surface: surface,
                connection_id: 5,
                passthrough: vec![],
            },
        );
        let child = app
            .world_mut()
            .spawn((
                InlineWebview {
                    view_id: "H".into(),
                    instance_id: None,
                    slot: 0,
                },
                WebviewSource::new("https://example.com"),
                ChildOf(surface),
            ))
            .id();
        app.insert_resource(reg);
        app.insert_resource(OzmuxRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Navigate {
                connection_id: 5,
                owner_surface: surface,
                handle: "H".into(),
                action: NavAction::To("https://example.com/next".into()),
            })
            .unwrap();
        app.update();

        match app.world().get::<WebviewSource>(child).unwrap() {
            WebviewSource::Url(u) => assert_eq!(u, "https://example.com/next"),
            other => panic!("expected Url, got {other:?}"),
        }
    }

    #[test]
    fn apply_navigate_back_triggers_request_go_back_on_owned_view() {
        use bevy_cef::prelude::{RequestGoBack, WebviewSource};
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let surface = app.world_mut().spawn_empty().id();
        let mut reg = DynamicRegistry::default();
        reg.insert(
            "H".into(),
            DynamicView {
                source: DynSource::Url {
                    url: "https://example.com".into(),
                    bridge: true,
                },
                entry: String::new(),
                interactive: true,
                owner_surface: surface,
                connection_id: 5,
                passthrough: vec![],
            },
        );
        let child = app
            .world_mut()
            .spawn((
                InlineWebview {
                    view_id: "H".into(),
                    instance_id: None,
                    slot: 0,
                },
                WebviewSource::new("https://example.com"),
                ChildOf(surface),
            ))
            .id();
        app.insert_resource(reg);
        app.insert_resource(OzmuxRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
        #[derive(Resource, Default)]
        struct BackOn(Vec<Entity>);
        app.insert_resource(BackOn::default());
        app.add_observer(|e: On<RequestGoBack>, mut c: ResMut<BackOn>| c.0.push(e.webview));
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Navigate {
                connection_id: 5,
                owner_surface: surface,
                handle: "H".into(),
                action: NavAction::Back,
            })
            .unwrap();
        app.update();

        assert_eq!(app.world().resource::<BackOn>().0, vec![child]);
    }

    #[test]
    fn apply_navigate_is_dropped_for_unowned_connection() {
        use bevy_cef::prelude::WebviewSource;
        let mut app = App::new();
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let surface = app.world_mut().spawn_empty().id();
        let mut reg = DynamicRegistry::default();
        reg.insert(
            "H".into(),
            DynamicView {
                source: DynSource::Url {
                    url: "https://example.com".into(),
                    bridge: true,
                },
                entry: String::new(),
                interactive: true,
                owner_surface: surface,
                connection_id: 5,
                passthrough: vec![],
            },
        );
        let child = app
            .world_mut()
            .spawn((
                InlineWebview {
                    view_id: "H".into(),
                    instance_id: None,
                    slot: 0,
                },
                WebviewSource::new("https://example.com"),
                ChildOf(surface),
            ))
            .id();
        app.insert_resource(reg);
        app.insert_resource(OzmuxRpc::default());
        app.insert_resource(ControlEvents(ev_rx));
        app.insert_resource(WebviewAssetRegistryRes(WebviewAssetRegistry::default()));
        app.add_systems(Update, apply_control_events);

        ev_tx
            .send(ControlEvent::Navigate {
                connection_id: 9, // not the owner (5)
                owner_surface: surface,
                handle: "H".into(),
                action: NavAction::To("https://evil.example/x".into()),
            })
            .unwrap();
        app.update();

        match app.world().get::<WebviewSource>(child).unwrap() {
            WebviewSource::Url(u) => assert_eq!(u, "https://example.com", "unowned navigate is a no-op"),
            other => panic!("expected Url, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p ozmux-gui control_plane::apply_tests::apply_navigate`
Expected: FAIL to compile — `apply_control_events` has no `Navigate` arm and the system lacks a `Query<&mut WebviewSource>`.

- [ ] **Step 3: Add imports + the `WebviewSource` query param**

In `src/control_plane.rs`, add to the import block (next to `use bevy_cef::prelude::FocusedWebview;`):

```rust
use bevy_cef::prelude::{RequestGoBack, RequestGoForward, RequestReload, WebviewSource};
```

Also bring `NavAction` into scope by extending `control_plane.rs`'s existing
import (line 7) from
`use crate::control_plane::protocol::{HostKeyChord, RegisterKind, ServerMsg};` to
`use crate::control_plane::protocol::{HostKeyChord, NavAction, RegisterKind, ServerMsg};`.

Add a `WebviewSource` query to `apply_control_events`'s parameter list (mutable params first — place it among the mutable params, after `mut focused`):

```rust
fn apply_control_events(
    mut commands: Commands,
    mut registry: ResMut<DynamicRegistry>,
    mut rpc: ResMut<OzmuxRpc>,
    mut focused: Option<ResMut<FocusedWebview>>,
    mut sources: Query<&mut WebviewSource>,
    events: Option<Res<ControlEvents>>,
    dyn_assets: Res<WebviewAssetRegistryRes>,
    inline: Query<(Entity, &InlineWebview)>,
    child_of: Query<&ChildOf>,
    non_interactive: Query<(), With<NonInteractive>>,
) {
```

- [ ] **Step 4: Add the `ControlEvent::Navigate` apply arm**

In the `match event` body of `apply_control_events`, add this arm (after `SetFocus`). It resolves ownership via the registry (connection_id only — NOT `is_bridged`), finds the mounted child by `view_id` + `owner_surface`, then performs the action; a helper keeps the body legible:

```rust
            ControlEvent::Navigate {
                connection_id,
                owner_surface,
                handle,
                action,
            } => {
                let Some(view) = registry.get(&handle) else {
                    continue;
                };
                if view.connection_id != connection_id {
                    tracing::debug!(handle = %handle, "navigate for unowned handle, dropping");
                    continue;
                }
                let is_url = matches!(view.source, DynSource::Url { .. });
                let target = inline.iter().find(|(entity, v)| {
                    v.view_id == handle
                        && child_of.get(*entity).map(|c| c.parent()) == Ok(owner_surface)
                });
                let Some((entity, _)) = target else {
                    tracing::debug!(handle = %handle, "navigate for unmounted view, dropping");
                    continue;
                };
                match action {
                    NavAction::To(url) => {
                        if !is_url {
                            tracing::debug!(handle = %handle, "navigate To on a non-url view, dropping");
                            continue;
                        }
                        match validate_url_source(&url) {
                            Ok(valid) => {
                                if let Ok(mut source) = sources.get_mut(entity) {
                                    *source = WebviewSource::Url(valid);
                                }
                            }
                            Err(e) => {
                                tracing::debug!(handle = %handle, error = e, "navigate To rejected url");
                            }
                        }
                    }
                    NavAction::Back => commands.trigger(RequestGoBack { webview: entity }),
                    NavAction::Forward => commands.trigger(RequestGoForward { webview: entity }),
                    NavAction::Reload => commands.trigger(RequestReload { webview: entity }),
                }
            }
```

- [ ] **Step 5: Run the apply tests to verify they pass**

Run: `cargo test -p ozmux-gui control_plane::apply_tests::apply_navigate`
Expected: PASS (3 new tests).

- [ ] **Step 6: Full host suite + commit**

Run: `cargo test -p ozmux-gui`
Expected: all PASS.

```bash
cargo fmt
git add src/control_plane.rs
git commit -m "feat(control-plane): apply Navigate via WebviewSource mutation + RequestGo* triggers"
```

---

### Task 4: ozbrowser — in-place navigation + remove its history stack

**Files:**
- Delete: `apps/ozbrowser/src/history.rs`
- Modify: `apps/ozbrowser/src/main.rs`
- Modify: `apps/ozbrowser/src/app.rs`
- Test: `apps/ozbrowser/src/app.rs`'s `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes (Task 1): `WebviewHandle::{navigate,go_back,go_forward,reload}`.
- Produces: ozbrowser navigates in place; the webview handle is registered once and stable for the session.

- [ ] **Step 1: Replace the history-coupled app tests (failing)**

In `apps/ozbrowser/src/app.rs`'s `mod tests`, DELETE every test that exercises the removed history API:
`go_back_with_empty_stack_returns_none`, `go_forward_with_empty_stack_returns_none`,
`go_back_navigates_to_previous_url`, `go_forward_after_back_restores_url`,
`navigate_updates_url_and_history`, `page_url_change_to_new_url_pushes_history`,
`page_url_change_to_current_url_is_noop`, `page_navigation_then_back_returns_to_previous_page`.

Keep `history_back_forward_produce_commands` (it asserts `on_action` → `Cmd`, which is unchanged). Add the simplified URL test:

```rust
    #[test]
    fn page_url_changed_updates_displayed_url() {
        let mut a = app();
        a.on_page_url_changed("https://docs.rs".into());
        assert_eq!(a.url(), "https://docs.rs");
    }
```

- [ ] **Step 2: Run to verify it fails to compile**

Run: `cargo test -p ozbrowser app`
Expected: FAIL — the kept/added tests still reference `App::navigate`/`go_back`/`go_forward` removed below; compile error after Step 3 edits resolve. (Run after Step 3.)

- [ ] **Step 3: Strip the history stack from `app.rs`**

In `apps/ozbrowser/src/app.rs`:

Remove the import line `use crate::history::History;`.

Remove the `history: History,` field from `struct App` and the `history: History::new(),` initializer in `App::new`.

Delete the three methods `navigate`, `go_back`, and `go_forward` entirely.

Simplify `on_page_url_changed` to set the URL only:

```rust
    /// Records a page-driven URL change reported via `urlChanged` (CEF owns the
    /// session history now, so this only updates the displayed URL).
    pub(crate) fn on_page_url_changed(&mut self, url: String) {
        self.url = url;
    }
```

- [ ] **Step 4: Delete `history.rs` and its module declaration**

```bash
git rm apps/ozbrowser/src/history.rs
```

In `apps/ozbrowser/src/main.rs`, remove the `mod history;` line.

- [ ] **Step 5: Make ozbrowser navigate in place in `main.rs`**

In `apps/ozbrowser/src/main.rs`:

`register_view` is already called once in `run()` before `event_loop`. Change `run()` to move (not clone) the senders into that single call and stop passing them to `event_loop`:

```rust
    let view = register_view(&ozma, &initial_url, url_tx, hint_tx)?;
```

Change the `event_loop` call to drop the sender args:

```rust
    let result = event_loop(view, App::new(initial_url), &ozma, &url_rx, &hint_rx);
```

Update `event_loop`'s signature: drop `url_tx`/`hint_tx`, make `view` immutable:

```rust
fn event_loop(
    view: WebviewHandle,
    mut app: App,
    ozma: &Ozma,
    url_rx: &Receiver<String>,
    hint_rx: &Receiver<String>,
) -> anyhow::Result<()> {
```

Replace the four re-registering `Cmd` arms with in-place calls (the `view = register_view(...)` reassignments are gone):

```rust
                    Cmd::Navigate(url) => {
                        let _ = view.navigate(url);
                    }
                    Cmd::HistoryBack => {
                        let _ = view.go_back();
                    }
                    Cmd::HistoryForward => {
                        let _ = view.go_forward();
                    }
                    Cmd::Reload => {
                        let _ = view.reload();
                    }
```

Leave the `Cmd::Scroll`, `Cmd::Hint*`, and `Cmd::Quit` arms unchanged.

- [ ] **Step 6: Build + run the app suite**

Run: `cargo build -p ozbrowser && cargo test -p ozbrowser`
Expected: builds clean (no unused `Sender`/`url_tx`/`hint_tx` warnings — they are now moved into `register_view`); all remaining tests PASS.

- [ ] **Step 7: Full workspace check + commit**

Run: `cargo test` and `cargo fmt`
Expected: workspace PASS.

```bash
git add apps/ozbrowser/src/main.rs apps/ozbrowser/src/app.rs
git commit -m "feat(ozbrowser): navigate the webview in place; drop the app-side history stack"
```

---

## Manual verification (after Task 4)

Run ozbrowser in an ozmux pane on a link-rich page and confirm:

1. `f` then follow a hint → the page navigates with **no webview disappear/reappear flicker**; the URL bar updates.
2. `H` returns to the previous page (no flicker); `L` goes forward; `r` reloads — all in place.
3. Address bar (`o`/`:`, type a URL, Enter) navigates in place (no flicker), and `H` afterward goes back to the prior page.
4. Rapid `H`/`L` does not blank the webview between steps.

## Self-Review notes (spec coverage)

- Spec §2 protocol (`ClientMsg::Navigate` + `NavAction` + SDK methods, control socket, mount-scoped) → Task 1 (SDK) + Task 2 (host wire).
- Spec §3 host apply (ownership = connection_id + owner_surface, not is_bridged; `To{url}` validated + `DynSource::Url`-only + `WebviewSource` mutation; Back/Forward/Reload → `Request*`; no registry sync) → Task 3.
- Spec §4 ozbrowser (register once, in-place ops, remove `History`, `on_page_url_changed` → set url) → Task 4.
- Spec §5 (keep host `AddressChanged→urlChanged`; remove app History) → Task 4 (the host producer is untouched).
- Spec §6/§8 limitations → no code (documented behavior; manual-verify list covers the happy path).
- Spec §7 testing → Task 1 protocol tests, Task 2 parse + listener tests, Task 3 apply tests, Task 4 app tests + manual list.
