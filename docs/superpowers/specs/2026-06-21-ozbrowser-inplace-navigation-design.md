# ozbrowser in-place navigation (CEF-native history)

Status: design (2026-06-21). Incorporates an adversarial Codex review of the
proposed approach.

## 1. Goal & problem

In `apps/ozbrowser`, `H`/`L` (history back/forward), address-bar `Navigate`, and
`Reload` currently each call `register_view()`, which mints a **new** webview
handle. The `ratatui-ozma` SDK's per-frame placement diff turns the handle-id
change into `unmount-inline;<old>` + `mount-inline;<new>`; the host despawns the
old CEF browser and spawns a fresh cold one, so the webview visibly disappears
and reappears (flicker), and every navigation leaks a never-unregistered handle.

Fix: navigate the **existing** webview in place. Delegate session history to
CEF's native history (model "b"); ozbrowser stops maintaining its own history
stack.

## 2. Control-socket protocol (not OSC)

Navigation is a state change on an already-mounted webview — like `emit`/`focus`
— so it rides the **control socket** (`ClientMsg`), not the OSC mount stream.
OSC (`mount-inline`/`unmount-inline`) stays purely for mount/placement/geometry.

Add one op in `sdk/ratatui-ozma/src/protocol.rs` (peer of `Emit`/`Focus`):

```rust
ClientMsg::Navigate { handle: String, action: NavAction }

enum NavAction {
    Back,
    Forward,
    Reload,
    To { url: String },
}
```

Add four `WebviewHandle` methods that write this over the same `SharedWriter`
`emit` uses: `navigate(url)` → `To{url}`, `go_back()` → `Back`,
`go_forward()` → `Forward`, `reload()` → `Reload`.

Like `emit`, `Navigate` is **mount-scoped**: a no-op (still `Ok`) when the
handle has no mounted view. This matches the existing focus/emit behavior and is
benign for ozbrowser, whose navigations are user keystrokes that occur well
after the startup mount (the documented focus-vs-mount race does not apply).

## 3. Host: apply navigation to the mounted webview

The control plane parses `ClientMsg::Navigate` into a `ControlEvent::Navigate`
and an apply system performs it, mirroring the existing `Emit` routing
(gated by the sending connection's ownership of the handle —
`registry.get(handle).connection_id == connection_id` plus its `owner_surface`,
the same scoping `Focus` uses — NOT by `is_bridged()`, so a display-only
`bridge:false` Url view is still navigable; an unowned/unmounted handle is a no-op):

1. Resolve `handle` → the mounted `InlineWebview` child entity for the sending
   connection.
2. Dispatch by `action`:
   - **`To { url }`** — validate `url` with the existing `validate_url_source`
     policy (`http(s)`, non-empty host); reject otherwise. Only a
     `DynSource::Url` registration may be navigated this way (`Dir`/`Inline`
     views derive their origin from the handle, so retargeting them would desync
     the served `ozma-dyn://` origin — ignore `To{url}` for those). Then mutate
     the mounted entity's `WebviewSource = WebviewSource::Url(url)`; bevy_cef
     reacts to `Changed<WebviewSource>` (`resolve_webview_source` →
     `navigate_on_source_change` → `browsers.navigate()`) and navigates the
     **existing** browser without recreating it. The write is event-driven (one
     per `Navigate`), not per-frame; `WebviewSource` derives no `PartialEq`, so
     assignment always fires `Changed` — the intended effect here, and it
     complies with the "mutate conditionally" rule because the apply runs only
     on a discrete `Navigate` event.
     - Rationale (Codex): firing bevy_cef's `RequestNavigate` would navigate the
       browser but leave the ECS `WebviewSource` stale; mutating `WebviewSource`
       is bevy_cef's intended in-place navigation path.
     - v1 does NOT sync the navigated URL back into the `DynamicRegistry` (it
       exposes no in-place URL mutation, and a partial sync would mislead —
       `Back`/`Forward`/link/hint navigations never update it either). See §6.
   - **`Back`** / **`Forward`** / **`Reload`** — these carry no URL, so trigger
     bevy_cef's native events on the entity:
     `commands.trigger(RequestGoBack { webview })` /
     `RequestGoForward { webview }` / `RequestReload { webview }`. bevy_cef's
     `apply_request_*` observers call `browsers.go_back/go_forward/reload` on the
     live browser (macOS path uses `NonSend<Browsers>`; the navigation plugin is
     added before the control plane, so the observers are present).

## 4. ozbrowser changes

- **Register once.** `register_view()` is called a single time at startup. The
  event loop no longer reassigns `view`; the handle is stable for the session.
  Its `.on("urlChanged")` / `.on("hintResult")` handlers and `passthrough` now
  persist across every navigation (more robust than per-nav re-registration, and
  this removes the never-unregistered-handle leak).
- **Map commands to in-place ops.** `Cmd::Navigate(url)` → `view.navigate(url)`;
  `Cmd::HistoryBack` → `view.go_back()`; `Cmd::HistoryForward` →
  `view.go_forward()`; `Cmd::Reload` → `view.reload()`.
- **Remove ozbrowser's own history.** Delete `apps/ozbrowser/src/history.rs`, the
  `App.history` field, and `App::{navigate, go_back, go_forward}`. `Cmd` keeps
  its `Navigate`/`HistoryBack`/`HistoryForward`/`Reload` variants (the keymap and
  `on_action` are unchanged); only their `main.rs` effects change.
- **URL bar from `urlChanged` only.** Simplify `App::on_page_url_changed(url)` to
  `self.url = url` (drop the history push — under CEF-native history, a
  back/forward `AddressChanged` echo would otherwise corrupt an app-side stack).

## 5. Relationship to the prior fix (commit `03ae955`)

- **Kept:** the host's `AddressChanged → urlChanged` producer
  (`src/webview_render.rs::on_webview_address_changed`). Under model (b) it is
  the **sole** URL-bar source, so it is load-bearing. Its gate depends only on
  the entity's `WebviewSource` **scheme** (`http(s)`), which is invariant for a
  given webview — `WebviewSource` is updated only on a host-initiated `To{url}`,
  not on `Back`/`Forward`/`Reload` or page-driven navigation, so its exact URL
  may be stale, but the scheme the gate relies on stays correct.
- **Removed / simplified:** ozbrowser's `History` and the history-push logic in
  `on_page_url_changed`.

## 6. Edge cases / accepted limitations

- **CEF history persists across normal use; lost only on genuine teardown.** The
  live browser holds the session history. It is destroyed only when the inline
  webview child is despawned: `unmount-inline`, program unregister/disconnect,
  tmux window **close**/reset, and alt-screen exit for `FixedScreen` children.
  Routine **resize**, **focus** changes, and tmux window **switching** do NOT
  despawn (they change `WebviewSize` or toggle display), so a single-pane
  ozbrowser session keeps its history throughout. Acceptable for v1.
- **Reload is `load_url(current)`, not a native CEF reload.** bevy_cef's
  `RequestReload` reloads by re-loading the current URL. A same-URL reload may
  not emit `AddressChanged`, so the URL bar will not flicker on reload — which is
  the correct outcome (the URL is unchanged); the page still reloads.
- **`can_go_back` / `can_go_forward` not surfaced (v1).** `H`/`L` are sent
  unconditionally; CEF no-ops at a history end (URL bar simply does not change).
  `AddressChanged` already carries these flags for a future UI affordance — out
  of scope here.
- **A remount reloads the registered URL, not the last-navigated one.** Because
  v1 does not sync the navigated URL back to the `DynamicRegistry` (§3), a torn-
  down-then-remounted view reloads its originally-registered URL. Consistent
  with the teardown history-loss above and out of scope (§8) for v1.

## 7. Testing

- **SDK** (`sdk/ratatui-ozma`): `WebviewHandle::{navigate,go_back,go_forward,reload}`
  serialize the expected `ClientMsg::Navigate { handle, action }` wire form
  (mirror the existing `emit`/register serialization tests, e.g.
  `webview.rs` `passthrough_rides_register_wire`). Protocol round-trip parse of
  each `NavAction`.
- **Host** (`ozmux-gui`): `ClientMsg::Navigate` parses; the apply path resolves a
  mounted, owned handle and (a) for `To{url}` sets the child's `WebviewSource` to
  `Url(url)` and updates the registry, (b) for `Back`/`Forward`/`Reload` triggers
  the corresponding `Request*` event; an unowned/unmounted handle is a no-op.
  Mirror the existing emit-routing apply tests.
- **ozbrowser**: after removing `History`, `on_action` still maps `H`/`L`/`r`/
  address-confirm to the right `Cmd`s; `on_page_url_changed` updates the
  displayed URL. (No app-side history tests remain.)
- **Manual**: in an ozmux pane, `f`/address-bar/`H`/`L`/`r` navigate with **no
  flicker** (the webview stays mounted), back/forward traverse real history, and
  the URL bar tracks the page.

## 8. Out of scope (YAGNI)

`can_go_back`/`can_go_forward` UI affordances, preserving CEF history across a
genuine unmount/remount, syncing the last-navigated URL back to the
`DynamicRegistry` for remount fidelity, multi-tab, and any change to the OSC
mount protocol.
