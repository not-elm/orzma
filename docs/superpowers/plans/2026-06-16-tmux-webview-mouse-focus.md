# tmux inline-webview mouse routing & focus — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an inline webview embedded in a tmux pane receive wheel/click/move (focus-based + pointer-gated), instead of the mouse wheel unconditionally entering tmux copy mode.

**Architecture:** Mirror the proven native inline-routing path (`src/input/mouse_buttons.rs` `route_inline_left_click`, `src/input/mouse_wheel.rs` `resolve_inline_wheel_target`, `src/inline_webview.rs` `forward_inline_mouse_moves`) into the three tmux input systems, keyed off `TmuxPane` hosts instead of `SurfaceMarker + Slotted`. Focus lives in the existing `FocusedWebview` resource, granted by an ungated `Browsers::set_focus` before the first click and preserved under tmux by a focus-validator change. Geometry helpers (`inline_hit_at`, `inline_local_dip`, `focused_inline_of`, `TerminalOverlays` projection) are reused unchanged.

**Tech Stack:** Rust 2024, Bevy 0.18 ECS, `bevy_cef` / `bevy_cef_core` (CEF webviews), `ozmux_tmux` (tmux control mode).

**Spec:** `docs/superpowers/specs/2026-06-16-tmux-webview-mouse-focus-design.md` — read it first.

---

## Reference reading (do this before Task 1)

Read these so the adaptations below are unambiguous:

- `src/input/mouse_buttons.rs`: `InlineRouteParams` (90-100), `resolve_pane_at_phys` (127), `phys_to_terminal_local` (159), `route_inline_left_click` (731-810), `inline_release_dip` (816-840).
- `src/input/mouse_wheel.rs`: `InlineWheelTarget` (≈170), `InlineWheelParams` (≈74), `aggregate_wheel_delta` (≈190), `inline_wheel_delta` (≈230), `resolve_inline_wheel_target` (≈252).
- `src/inline_webview.rs`: `focused_inline_of` (352), `InlineHit` + `inline_hit_at` (365-424), `inline_local_dip` (432), `forward_inline_mouse_moves` (563-609).
- `src/tmux_mouse.rs`: `OzmuxTmuxMousePlugin` (43-48), `TmuxMouseGesture` (104-108), `pane_under_cursor` (135-143), the `arbiter` modal guard (227-244) and its `for ev in buttons.read()` press/release loop (266-415).
- `src/tmux_input.rs`: `OzmuxTmuxInputPlugin` (32-46), `forward_wheel_to_tmux` (392-482).
- `src/webview_render.rs`: `sync_focused_webview` (93-114).

## File structure

All changes are tmux-local; native path and shared helpers are untouched.

- `src/webview_render.rs` — Task 1: tmux-aware focus preservation/GC in `sync_focused_webview`.
- `src/tmux_mouse.rs` — Task 2: `tmux_pane_local_at` helper, `inline_press` field on `TmuxMouseGesture`, `TmuxInlineRouteParams`, `route_tmux_inline_left_click`, `tmux_inline_release_dip`, arbiter wiring, modal-guard change. Task 3: `forward_tmux_inline_mouse_moves` system + plugin registration.
- `src/tmux_input.rs` — Task 4: `TmuxInlineWheelTarget`, `TmuxInlineWheelParams`, `resolve_tmux_inline_wheel_target`, `aggregate_tmux_wheel_cells`, `forward_wheel_to_tmux` rewrite.

Run one crate's worth of tests with `cargo test --bin ozmux-gui <name>` (these systems live in the root binary, `ozmux-gui`).

---

## Task 1: tmux-aware focus validator

`sync_focused_webview` clears `FocusedWebview` every frame unless the focused inline child's parent is the **old-multiplexer** active surface — which the tmux backend never populates, so a tmux-pane inline focus is clobbered one frame after a click sets it. Add a preservation arm: keep focus while it points at an inline child of a live `TmuxPane`. Despawn GC still works (the existing fall-through clears focus when the child entity is gone).

**Files:**
- Modify: `src/webview_render.rs:93-114` (`sync_focused_webview`)
- Test: `src/webview_render.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/webview_render.rs` (it already uses `RunSystemOnce`; mirror the existing `focused_webview_follows_active_pane` test for setup of `FocusedWebview`, `InlineWebview`, and a parent entity). Use `ozmux_tmux::TmuxPane`.

```rust
#[test]
fn tmux_pane_inline_focus_is_preserved() {
    use ozmux_tmux::{PaneId, TmuxPane};
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(ozmux_multiplexer::MultiplexerPlugin);
    app.init_resource::<FocusedWebview>();
    app.add_systems(Update, sync_focused_webview);

    // A TmuxPane host with an InlineWebview child; no old-mux active surface.
    let pane = app.world_mut().spawn(TmuxPane::new(PaneId(1))).id();
    let child = app
        .world_mut()
        .spawn((
            ChildOf(pane),
            InlineWebview { view_id: "v".into(), instance_id: None, slot: 0 },
        ))
        .id();
    app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);

    app.update();

    assert_eq!(
        app.world().resource::<FocusedWebview>().0,
        Some(child),
        "an inline child of a live TmuxPane must keep FocusedWebview across the per-frame sync",
    );
}

#[test]
fn tmux_pane_inline_focus_is_gc_on_despawn() {
    use ozmux_tmux::{PaneId, TmuxPane};
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(ozmux_multiplexer::MultiplexerPlugin);
    app.init_resource::<FocusedWebview>();
    app.add_systems(Update, sync_focused_webview);

    let pane = app.world_mut().spawn(TmuxPane::new(PaneId(1))).id();
    let child = app
        .world_mut()
        .spawn((
            ChildOf(pane),
            InlineWebview { view_id: "v".into(), instance_id: None, slot: 0 },
        ))
        .id();
    app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
    app.world_mut().entity_mut(child).despawn();

    app.update();

    assert_eq!(
        app.world().resource::<FocusedWebview>().0, None,
        "a despawned inline child must be GC'd out of FocusedWebview",
    );
}
```

> NOTE: confirm `TmuxPane`'s constructor — if `TmuxPane::new(PaneId)` does not exist, build it the way the existing `tmux_mouse.rs`/`tmux_render.rs` tests construct a `TmuxPane` (grep `TmuxPane {` / `TmuxPane::`). The `InlineWebview` literal fields are `view_id`, `instance_id`, `slot` (see `src/inline_webview.rs:44-55`).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --bin ozmux-gui tmux_pane_inline_focus -- --test-threads=1`
Expected: FAIL — `tmux_pane_inline_focus_is_preserved` asserts `Some(child)` but the current sync clears it to `None`.

- [ ] **Step 3: Implement the preservation arm**

Add the import (in the existing top-of-file `use` block, no blank-line grouping):

```rust
use ozmux_tmux::TmuxPane;
```

Add a `tmux_panes` query parameter and the preservation arm at the top of `sync_focused_webview` (before the old-mux `active_surface` resolution). Mutable params stay first:

```rust
pub(crate) fn sync_focused_webview(
    mut focused: ResMut<FocusedWebview>,
    mux: MultiplexerCommands,
    attached_workspace: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    webviews: Query<(), With<WebviewSource>>,
    non_interactive: Query<(), With<NonInteractive>>,
    inline_parents: Query<&ChildOf, With<InlineWebview>>,
    tmux_panes: Query<(), With<TmuxPane>>,
) {
    // Under the tmux backend FocusedWebview is click-driven, not active-pane
    // driven: preserve it while it points at an inline child of a live TmuxPane.
    // A despawned child fails `inline_parents.get` and falls through to the
    // old-mux path below, which clears it (GC).
    if let Some(child) = focused.0
        && let Ok(parent) = inline_parents.get(child)
        && tmux_panes.contains(parent.parent())
    {
        return;
    }

    let active_surface = attached_workspace
        .iter()
        .next()
        .and_then(|workspace| mux.workspaces_active_pane(workspace))
        .and_then(|pane| mux.panes_active_surface(pane));
    if focused_inline_of(Some(&focused), &inline_parents, active_surface).is_some() {
        return;
    }
    let active = active_surface
        .filter(|surface| webviews.contains(*surface) && !non_interactive.contains(*surface));
    if focused.0 != active {
        focused.0 = active;
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --bin ozmux-gui tmux_pane_inline_focus -- --test-threads=1`
Expected: PASS (both tests).
Also run the existing regression: `cargo test --bin ozmux-gui focused_webview_follows_active_pane -- --test-threads=1` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/webview_render.rs
git commit -m "feat(tmux): preserve click-driven inline-webview focus under the tmux backend"
```

---

## Task 2: arbiter inline click routing + `inline_press` marker

Route a left press inside an interactive inline rect to the webview (focus + click), consuming it so it never arms divider-drag/drag-select; route the matching release to the same child. Drop the blanket `focused_webview` drain from the modal guard (inline routing now owns focus).

**Files:**
- Modify: `src/tmux_mouse.rs` — add `tmux_pane_local_at`, `TmuxInlineRouteParams`, `route_tmux_inline_left_click`, `tmux_inline_release_dip`; add `inline_press` to `TmuxMouseGesture`; wire into `arbiter`; change the modal guard.
- Test: `src/tmux_mouse.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Add imports + the `inline_press` field + the pane-local helper**

Add to the top `use` block (no blank-line grouping). Mirror native import paths:

```rust
use crate::input::mouse_buttons::phys_to_terminal_local;
use crate::inline_webview::{InlineHit, InlineWebview, inline_hit_at, inline_local_dip};
use crate::osc_webview::NonInteractive;
use bevy::ecs::system::SystemParam;
use bevy_cef::prelude::{FocusedWebview, PointerButton};
use bevy_cef_core::prelude::Browsers;
use ozma_tty_renderer::prelude::TerminalOverlays;
```

> NOTE: `FocusedWebview` is already imported at `src/tmux_mouse.rs:31` — extend that line to `use bevy_cef::prelude::{FocusedWebview, PointerButton};` rather than adding a duplicate. Confirm `PointerButton` is re-exported from `bevy_cef::prelude` (it is the type `route_inline_left_click` uses in `mouse_buttons.rs`; match whatever path that file imports).

Add the field to `TmuxMouseGesture` (`src/tmux_mouse.rs:104-108`):

```rust
#[derive(Resource, Default)]
pub(crate) struct TmuxMouseGesture {
    state: GestureState,
    click: ClickTracker,
    /// The in-flight inline-webview press (mirrors native
    /// `MouseSelectionState.inline_press`): the child a left press inside an
    /// interactive inline rect was forwarded to, so the matching release's
    /// click-up routes to the SAME child even if the pointer drifted off-rect.
    inline_press: Option<Entity>,
}
```

Add the pane-local resolver (private helper, placed with the other free functions near `pane_under_cursor`):

```rust
/// Resolves the `TmuxPane` terminal entity under `cursor_phys` (physical px)
/// and the pointer in that pane's terminal-local physical px, or `None` when
/// no pane covers the point. The tmux analog of native `resolve_pane_at_phys`.
fn tmux_pane_local_at(
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    cursor_phys: Vec2,
) -> Option<(Entity, Vec2)> {
    panes.iter().find_map(|(entity, _, node, transform)| {
        if !node.contains_point(*transform, cursor_phys) {
            return None;
        }
        let local = phys_to_terminal_local(node, transform, cursor_phys)?;
        Some((entity, local))
    })
}
```

- [ ] **Step 2: Write the failing test (press inside rect focuses + consumes)**

Add a fixture + test to `src/tmux_mouse.rs` tests. Model the fixture on `tmux_render.rs`/`mouse_wheel.rs` (`make_wheel_app`): a `TmuxPane` host at a known node, a `TerminalOverlays` with one rect, and an `InlineWebview` child. Build the `arbiter` app with the resources it reads (`TmuxMouseGesture`, `TmuxConnection`, `CopyModeQueries`, `SessionPicker`, `CopyPrompt`, `FocusedWebview`, metrics, a focused `PrimaryWindow`, `Time<Real>`), then drive a left press at a cursor inside the rect.

```rust
#[test]
fn inline_press_focuses_child_and_consumes() {
    let (mut app, _pane, child) = make_arbiter_inline_app(); // helper below
    set_cursor(&mut app, Vec2::new(40.0, 48.0)); // inside the rect (see fixture)
    write_button(&mut app, MouseButton::Left, ButtonState::Pressed);

    app.update();

    assert_eq!(
        app.world().resource::<FocusedWebview>().0, Some(child),
        "a press inside an interactive inline rect must focus that child",
    );
    assert_eq!(
        app.world().resource::<TmuxMouseGesture>().state,
        GestureState::Idle,
        "a consumed inline press must NOT arm a Pressed/Selecting gesture",
    );
}

#[test]
fn inline_off_rect_press_releases_focus_and_falls_through() {
    let (mut app, pane, child) = make_arbiter_inline_app();
    app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
    set_cursor(&mut app, Vec2::new(400.0, 400.0)); // inside the pane node, outside the rect

    write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
    app.update();

    assert_eq!(
        app.world().resource::<FocusedWebview>().0, None,
        "an off-rect press must release inline focus",
    );
    // The fall-through path focuses the pane: state becomes Pressed on that pane.
    assert!(
        matches!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Pressed { pane: p, .. } if p == pane
        ),
        "an off-rect press must fall through to the normal pane gesture",
    );
}
```

The fixture (CEF-less: no `Browsers`, so state effects apply but no CEF forward) — add to the tests module:

```rust
fn make_arbiter_inline_app() -> (App, Entity, Entity) {
    use crate::inline_webview::InlineWebview;
    use ozma_tty_renderer::prelude::TerminalOverlays;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<MouseButtonInput>();
    app.insert_non_send_resource(TmuxConnection::default());
    app.init_resource::<TmuxMouseGesture>();
    app.init_resource::<CopyModeQueries>();
    app.init_resource::<SessionPicker>();
    app.init_resource::<CopyPrompt>();
    app.init_resource::<FocusedWebview>();
    app.insert_resource(test_metrics()); // existing helper, 8x16 px cells
    app.add_systems(Update, arbiter);

    // Pane node: window-centered 800x600 at (400,300) → top-left (0,0), like make_wheel_app.
    let pane = app
        .world_mut()
        .spawn((
            tmux_pane_fixture(1, 100, 37), // see NOTE — width/height in cells
            ComputedNode { size: Vec2::new(800.0, 600.0), ..ComputedNode::DEFAULT },
            UiGlobalTransform::from_xy(400.0, 300.0),
            TerminalOverlays::default(),
        ))
        .id();
    // Rect rows 2..12, cols 3..43 → phys y 32..192, x 24..344 at 8x16 px.
    app.world_mut().get_mut::<TerminalOverlays>(pane).unwrap().rects[0] =
        IVec4::new(2, 3, 10, 40);
    let child = app
        .world_mut()
        .spawn((
            ChildOf(pane),
            InlineWebview { view_id: "v".into(), instance_id: None, slot: 0 },
        ))
        .id();

    app.world_mut().spawn((
        Window { focused: true, resolution: WindowResolution::new(800, 600), ..default() },
        PrimaryWindow,
    ));
    (app, pane, child)
}

fn set_cursor(app: &mut App, phys: Vec2) {
    let win = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .get_mut::<Window>(win)
        .unwrap()
        .set_physical_cursor_position(Some(bevy::math::DVec2::new(phys.x as f64, phys.y as f64)));
}

fn write_button(app: &mut App, button: MouseButton, state: ButtonState) {
    app.world_mut()
        .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
        .write(MouseButtonInput { button, state, window: Entity::PLACEHOLDER });
}
```

> NOTE: `tmux_pane_fixture(id, w, h)` / `tmux_pane_fixture` is a placeholder — construct a `TmuxPane` with `dims.width`/`dims.height` set (so `cell_at_pane` and the fall-through pane-focus work) the same way the existing `tmux_mouse.rs` tests build one. Grep `TmuxPane {` in the test module; if there is no existing constructor, build the struct literal. The pane must carry `TmuxPane`, `ComputedNode`, `UiGlobalTransform`, and `TerminalOverlays` so both `pane_under_cursor` and `tmux_pane_local_at` resolve it.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --bin ozmux-gui inline_press_focuses_child_and_consumes inline_off_rect -- --test-threads=1`
Expected: FAIL — `arbiter` does not yet route inline clicks; `FocusedWebview` stays unchanged and the gesture arms `Pressed` on the in-rect press.

- [ ] **Step 4: Add the route helpers + release-dip + wire the arbiter**

Add the SystemParam bundle and helpers (private; place after `pane_under_cursor`). This mirrors native `InlineRouteParams` / `route_inline_left_click` / `inline_release_dip`:

```rust
/// Inline-routing params for the arbiter, bundled to stay within Bevy's
/// system-parameter limit. `focused_webview` / `browsers` are optional so
/// CEF-less tests construct the system (state effects still apply).
#[derive(SystemParam)]
struct TmuxInlineRouteParams<'w, 's> {
    focused_webview: Option<ResMut<'w, FocusedWebview>>,
    children: Query<'w, 's, &'static Children>,
    inline: Query<'w, 's, (&'static InlineWebview, Has<NonInteractive>)>,
    inline_parents: Query<'w, 's, &'static ChildOf, With<InlineWebview>>,
    overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
    browsers: Option<NonSend<'w, Browsers>>,
}

/// Routes a left press/release through the inline-webview layer, returning
/// `true` when the event was consumed and must NOT reach the tmux gesture
/// pipeline. Mirrors native `route_inline_left_click`: a press inside an
/// interactive rect sets `FocusedWebview`, issues the UNGATED `set_focus`
/// BEFORE the gated `send_mouse_click` (CEF drops clicks to a browser with no
/// `focused_frame()`, so the first click would otherwise be swallowed),
/// forwards the press in DIP, and records the in-flight press; a press outside
/// every rect clears an inline `FocusedWebview` and returns `false`. Release
/// forwards the click-up to the recorded child (drift-tolerant) and clears.
#[allow(clippy::too_many_arguments)]
fn route_tmux_inline_left_click(
    gesture: &mut TmuxMouseGesture,
    route: &mut TmuxInlineRouteParams,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    terminal: Entity,
    local_phys: Vec2,
    cursor_phys: Vec2,
    button_state: ButtonState,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) -> bool {
    match button_state {
        ButtonState::Pressed => {
            // A fresh press invalidates any stale in-flight marker (its release
            // was lost, e.g. released off-window where the arbiter early-returns).
            gesture.inline_press = None;
            let hit = route.overlay_rects.get(terminal).ok().and_then(|overlays| {
                inline_hit_at(
                    &route.children, &route.inline, overlays, terminal,
                    local_phys, cell_w_phys, cell_h_phys, scale,
                )
            });
            let Some(hit) = hit else {
                if let Some(focused) = route.focused_webview.as_deref_mut()
                    && focused.0.is_some_and(|c| route.inline_parents.contains(c))
                {
                    focused.0 = None;
                }
                return false;
            };
            if let Some(focused) = route.focused_webview.as_deref_mut()
                && focused.0 != Some(hit.child)
            {
                focused.0 = Some(hit.child);
            }
            if let Some(browsers) = route.browsers.as_deref() {
                browsers.set_focus(&hit.child, true);
                browsers.send_mouse_click(&hit.child, hit.local_dip, PointerButton::Primary, false);
            }
            gesture.inline_press = Some(hit.child);
            true
        }
        ButtonState::Released => {
            let Some(child) = gesture.inline_press.take() else {
                return false;
            };
            if let Some(browsers) = route.browsers.as_deref()
                && let Some(dip) = tmux_inline_release_dip(
                    route, panes, child, cursor_phys, cell_w_phys, cell_h_phys, scale,
                )
            {
                browsers.send_mouse_click(&child, dip, PointerButton::Primary, true);
            }
            true
        }
    }
}

/// Webview-local DIP for a release on `child`, WITHOUT containment (a pointer
/// that drifted off the rect still produces a release position). `None` when
/// the child/terminal/rect chain is gone. The tmux analog of native
/// `inline_release_dip`.
fn tmux_inline_release_dip(
    route: &TmuxInlineRouteParams,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    child: Entity,
    cursor_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) -> Option<Vec2> {
    let terminal = route.inline_parents.get(child).ok()?.parent();
    let (_, _, node, transform) = panes.get(terminal).ok()?;
    let local_phys = phys_to_terminal_local(node, transform, cursor_phys)?;
    let (view, _) = route.inline.get(child).ok()?;
    inline_local_dip(
        route.overlay_rects.get(terminal).ok()?,
        view.slot, local_phys, cell_w_phys, cell_h_phys, scale,
    )
}
```

Now wire it into `arbiter`. Add `mut inline_route: TmuxInlineRouteParams` to the signature and **replace** the `focused_webview: Res<FocusedWebview>` parameter (the bundle owns it now). Change the modal guard (lines 240-244) to drop the `focused_webview` term:

```rust
    // NOTE: a gesture behind a picker / copy-search prompt must not mutate
    // tmux. The focused-webview case is NOT drained here — the inline click
    // pre-step below owns focus (in-rect press keeps it, off-rect press
    // releases it and drives tmux).
    if picker.open || copy_prompt.open.is_some() {
        buttons.clear();
        gesture.state = GestureState::Idle;
        return;
    }
```

Inside `for ev in buttons.read()`, at the very top of the loop body (after the `if ev.button != Left { continue; }` guard, before the `match ev.state`), offer the event to the inline layer first:

```rust
        if ev.button != bevy::input::mouse::MouseButton::Left {
            continue;
        }
        if let Some(cursor_phys) = window.cursor_position().map(|c| c * scale)
            && let Some((terminal, local_phys)) = tmux_pane_local_at(&panes, cursor_phys)
            && route_tmux_inline_left_click(
                &mut gesture, &mut inline_route, &panes, terminal, local_phys,
                cursor_phys, ev.state, cell_w, cell_h, scale,
            )
        {
            continue; // consumed by the inline layer; skip the tmux gesture pipeline
        }
        match ev.state { /* ... existing Pressed / Released arms unchanged ... */ }
```

> NOTE: `gesture` is `ResMut<TmuxMouseGesture>`; pass `&mut gesture` (deref). The existing arms read `gesture.state` / `gesture.click` — borrow-checker: the inline call finishes (returns `bool`) before `match ev.state`, so there is no overlapping borrow. If the borrow checker complains about `gesture` being borrowed across the `for` body, hoist `cursor_phys`/`tmux_pane_local_at` results into locals computed before the call.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --bin ozmux-gui inline_press_focuses_child_and_consumes inline_off_rect -- --test-threads=1`
Expected: PASS (both).

- [ ] **Step 6: Regression — existing arbiter tests still pass**

Run: `cargo test --bin ozmux-gui --  --test-threads=1 tmux_mouse`
(Or run the whole binary's tmux mouse tests.) Expected: existing tests (`gesture_state_default_is_idle`, divider hit-tests, click-count, `left_press_without_cursor_stays_idle`, multi-select) PASS unchanged.

- [ ] **Step 7: Commit**

```bash
git add src/tmux_mouse.rs
git commit -m "feat(tmux): route inline-webview clicks (focus + CEF) in the mouse arbiter"
```

---

## Task 3: tmux inline move/hover/drag forwarder

A single stateless `CursorMoved`-driven system that forwards pointer motion over an interactive inline rect to CEF, forwarding whatever buttons are held (so hover and in-rect drag both work). Exact mirror of native `forward_inline_mouse_moves`, but over `TmuxPane` hosts.

**Files:**
- Modify: `src/tmux_mouse.rs` — add `forward_tmux_inline_mouse_moves`; register it in `OzmuxTmuxMousePlugin`.
- Test: `src/tmux_mouse.rs` tests

- [ ] **Step 1: Write the failing test (routing decision via hit-test)**

Because CEF forwarding needs `Browsers` (absent in tests), assert the *resolution* the system performs — extract the resolution into a testable pure helper and test that.

```rust
#[test]
fn move_resolves_inline_child_over_rect() {
    let (mut app, _pane, child) = make_arbiter_inline_app();
    let hit = app
        .world_mut()
        .run_system_once(move |
            panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
            children: Query<&Children>,
            inline: Query<(&InlineWebview, Has<NonInteractive>)>,
            overlays: Query<&TerminalOverlays>,
        | {
            let (terminal, local) = tmux_pane_local_at(&panes, Vec2::new(40.0, 48.0)).unwrap();
            inline_hit_at(&children, &inline, overlays.get(terminal).unwrap(),
                terminal, local, 8.0, 16.0, 1.0).map(|h| h.child)
        })
        .unwrap();
    assert_eq!(hit, Some(child), "pointer over the rect must resolve the inline child");
}

#[test]
fn move_resolves_nothing_off_rect() {
    let (mut app, _pane, _child) = make_arbiter_inline_app();
    let hit = app
        .world_mut()
        .run_system_once(|
            panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
            children: Query<&Children>,
            inline: Query<(&InlineWebview, Has<NonInteractive>)>,
            overlays: Query<&TerminalOverlays>,
        | {
            let (terminal, local) = tmux_pane_local_at(&panes, Vec2::new(400.0, 400.0)).unwrap();
            inline_hit_at(&children, &inline, overlays.get(terminal).unwrap(),
                terminal, local, 8.0, 16.0, 1.0).map(|h| h.child)
        })
        .unwrap();
    assert_eq!(hit, None, "pointer over terminal text must resolve no inline child");
}
```

- [ ] **Step 2: Run to verify it fails (compile error — system not yet present / imports)**

Run: `cargo test --bin ozmux-gui move_resolves -- --test-threads=1`
Expected: FAIL to compile until `RunSystemOnce` is imported in the test module (`use bevy::ecs::system::RunSystemOnce;`) — add it; then the test compiles and (since `tmux_pane_local_at`/`inline_hit_at` already exist from Task 2) it should PASS. If it already passes, that is fine — these tests pin the resolution helper; proceed to add the system itself.

- [ ] **Step 3: Implement the forwarder system**

Add to `src/tmux_mouse.rs`. Mirror `forward_inline_mouse_moves` (`src/inline_webview.rs:563`). Read `CursorMoved`, skip while a modal owns input, resolve the pane + rect, forward held buttons:

```rust
/// Forwards pointer motion over an interactive inline rect of a tmux pane to
/// the child's CEF browser (`send_mouse_move`, webview-local DIP), forwarding
/// whatever mouse buttons are held so the one system serves both hover and an
/// in-rect drag. The tmux analog of native `forward_inline_mouse_moves`:
/// `CursorMoved`-driven (one forward per frame, latest position), and
/// focus-gated inside `bevy_cef` so motion over an unfocused browser is
/// dropped browser-side. `Browsers` is optional so CEF-less tests construct it.
fn forward_tmux_inline_mouse_moves(
    mut cursor_msg: MessageReader<CursorMoved>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    children: Query<&Children>,
    inline: Query<(&InlineWebview, Has<NonInteractive>)>,
    overlay_rects: Query<&TerminalOverlays>,
    windows: Query<&Window, With<PrimaryWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    picker: Res<SessionPicker>,
    copy_prompt: Res<CopyPrompt>,
    browsers: Option<NonSend<Browsers>>,
) {
    let Some(moved) = cursor_msg.read().last() else {
        return;
    };
    if picker.open || copy_prompt.open.is_some() {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let scale = window.scale_factor();
    let cursor_phys = moved.position * scale;
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let Some((terminal, local_phys)) = tmux_pane_local_at(&panes, cursor_phys) else {
        return;
    };
    let Ok(overlays) = overlay_rects.get(terminal) else {
        return;
    };
    let Some(hit) = inline_hit_at(
        &children, &inline, overlays, terminal, local_phys, cell_w, cell_h, scale,
    ) else {
        return;
    };
    if let Some(browsers) = browsers.as_ref() {
        browsers.send_mouse_move(&hit.child, mouse_buttons.get_pressed(), hit.local_dip, false);
    }
}
```

Register it in `OzmuxTmuxMousePlugin::build` (mirror native: `InputPhase::Hover`):

```rust
impl Plugin for OzmuxTmuxMousePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TmuxMouseGesture>();
        app.add_systems(Update, arbiter.in_set(InputPhase::Dispatch));
        app.add_systems(Update, forward_tmux_inline_mouse_moves.in_set(InputPhase::Hover));
    }
}
```

> NOTE: ensure `MouseButton`, `CursorMoved`, and `ButtonInput` are imported (the arbiter already uses `MouseButton`; add `bevy::input::mouse::CursorMoved` and `bevy::input::ButtonInput` if not in scope — grep the existing top imports).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --bin ozmux-gui move_resolves -- --test-threads=1`
Expected: PASS. Also `cargo build --bin ozmux-gui` to confirm the system registers and compiles.

- [ ] **Step 5: Commit**

```bash
git add src/tmux_mouse.rs
git commit -m "feat(tmux): forward hover/drag over inline-webview rects to CEF"
```

---

## Task 4: pointer-gated inline wheel

Forward the wheel to a focused inline webview only when the pointer is over its rect; otherwise run the existing tmux path. Remove the blanket `focused_webview.0.is_some()` early-return.

**Files:**
- Modify: `src/tmux_input.rs` — add `TmuxInlineWheelTarget`, `TmuxInlineWheelParams`, `resolve_tmux_inline_wheel_target`, `aggregate_tmux_wheel_cells`; rewrite the head of `forward_wheel_to_tmux`.
- Test: `src/tmux_input.rs` tests

- [ ] **Step 1: Add imports + the target type + params bundle + resolver**

Add to the top `use` block of `src/tmux_input.rs` (extend the existing `use bevy_cef::prelude::FocusedWebview;` line and add the others):

```rust
use crate::input::mouse_buttons::phys_to_terminal_local;
use crate::inline_webview::{InlineWebview, focused_inline_of, inline_hit_at};
use crate::osc_webview::NonInteractive;
use bevy::ecs::system::SystemParam;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy_cef::prelude::FocusedWebview;
use bevy_cef_core::prelude::Browsers;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozmux_tmux::TmuxPane;
```

> NOTE: `tmux_pane_local_at` is private to `tmux_mouse.rs`. Either (a) make it `pub(crate)` in `tmux_mouse.rs` and import it here, or (b) duplicate the 6-line resolver locally. Prefer (a): change `fn tmux_pane_local_at` to `pub(crate) fn tmux_pane_local_at` in Task 2 and `use crate::tmux_mouse::tmux_pane_local_at;` here.

Add the target + resolver (mirror native `InlineWheelTarget` / `resolve_inline_wheel_target` / `inline_wheel_delta`):

```rust
/// A focused inline webview claiming the wheel: the child to forward to and
/// the pointer in its webview-local DIP.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TmuxInlineWheelTarget {
    child: Entity,
    position_dip: Vec2,
}

/// Wheel-routing params bundled to stay within Bevy's system-parameter limit.
#[derive(SystemParam)]
struct TmuxInlineWheelParams<'w, 's> {
    focused_webview: Res<'w, FocusedWebview>,
    inline_parents: Query<'w, 's, &'static ChildOf, With<InlineWebview>>,
    panes: Query<'w, 's, (Entity, &'static TmuxPane, &'static ComputedNode, &'static UiGlobalTransform)>,
    children: Query<'w, 's, &'static Children>,
    inline: Query<'w, 's, (&'static InlineWebview, Has<NonInteractive>)>,
    overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
    browsers: Option<NonSend<'w, Browsers>>,
}

/// Resolves the focused inline webview under the pointer, or `None` (the tmux
/// path runs). `Some` only when `FocusedWebview` holds an inline child of the
/// pane under the pointer AND the pointer is over that child's rect — exactly
/// native `resolve_inline_wheel_target`, over TmuxPane hosts.
fn resolve_tmux_inline_wheel_target(
    params: &TmuxInlineWheelParams,
    cursor_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale_factor: f32,
) -> Option<TmuxInlineWheelTarget> {
    let (terminal, local_phys) = crate::tmux_mouse::tmux_pane_local_at(&params.panes, cursor_phys)?;
    let focused_child = focused_inline_of(Some(&params.focused_webview), &params.inline_parents, Some(terminal))?;
    let overlays = params.overlay_rects.get(terminal).ok()?;
    let hit = inline_hit_at(
        &params.children, &params.inline, overlays, terminal,
        local_phys, cell_w_phys, cell_h_phys, scale_factor,
    )?;
    (hit.child == focused_child).then_some(TmuxInlineWheelTarget {
        child: hit.child,
        position_dip: hit.local_dip,
    })
}

/// Converts one `MouseWheel` event to the RAW CEF wheel delta (`Line → ×120`,
/// `Pixel` unchanged, NO sign flip) — identical to native `inline_wheel_delta`.
fn tmux_inline_wheel_delta(unit: MouseScrollUnit, x: f32, y: f32) -> Vec2 {
    match unit {
        MouseScrollUnit::Line => Vec2::new(x, y) * 120.0,
        MouseScrollUnit::Pixel => Vec2::new(x, y),
    }
}
```

- [ ] **Step 2: Write the failing test (target resolution)**

Mirror `mouse_wheel.rs`'s `target_resolves_when_focused_inline_under_pointer` / `target_none_when_*`. Build a `TmuxPane` + `TerminalOverlays` + `InlineWebview` fixture (reuse the Task 2 fixture shape) and run `resolve_tmux_inline_wheel_target` via `RunSystemOnce`.

```rust
#[test]
fn wheel_target_resolves_when_focused_inline_under_pointer() {
    let (mut app, _pane, child) = make_tmux_wheel_app(); // fixture mirroring make_arbiter_inline_app
    app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
    let target = run_resolve_wheel_target(&mut app, Vec2::new(40.0, 48.0));
    assert_eq!(target.map(|t| t.child), Some(child));
}

#[test]
fn wheel_target_none_off_rect() {
    let (mut app, _pane, child) = make_tmux_wheel_app();
    app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
    assert_eq!(run_resolve_wheel_target(&mut app, Vec2::new(400.0, 400.0)), None);
}

#[test]
fn wheel_target_none_when_unfocused() {
    let (mut app, _pane, _child) = make_tmux_wheel_app(); // FocusedWebview stays None
    assert_eq!(run_resolve_wheel_target(&mut app, Vec2::new(40.0, 48.0)), None);
}
```

`run_resolve_wheel_target` runs the resolver with the cursor set; provide cell metrics 8x16, scale 1:

```rust
fn run_resolve_wheel_target(app: &mut App, cursor_phys: Vec2) -> Option<TmuxInlineWheelTarget> {
    app.world_mut()
        .run_system_once(move |params: TmuxInlineWheelParams| {
            resolve_tmux_inline_wheel_target(&params, cursor_phys, 8.0, 16.0, 1.0)
        })
        .unwrap()
}
```

> NOTE: `make_tmux_wheel_app` is the same fixture as Task 2's `make_arbiter_inline_app` minus the `arbiter` system — factor the fixture into a shared test helper if both modules need it, or duplicate it in this module's tests.

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test --bin ozmux-gui wheel_target_ -- --test-threads=1`
Expected: FAIL to compile (resolver/types not yet referenced by a test) → then FAIL/PASS as the resolver is added. After Step 1 they should compile; the assertions pass once the resolver is correct. If `wheel_target_none_when_unfocused` fails, check `focused_inline_of` is passed `Some(terminal)` as `active_surface`.

- [ ] **Step 4: Rewrite the head of `forward_wheel_to_tmux`**

Add `inline: TmuxInlineWheelParams` to the signature. At the top (after computing `dpr`, `cell_h_logical`, and physical cell metrics), resolve the target, forward inline events, and drop the `focused_webview` term from the modal guard. Replace the existing aggregate+guard prologue:

```rust
    let dpr = window.scale_factor().max(0.5);
    let cell_h_logical = (metrics.metrics.line_height_phys.floor() / dpr).max(1.0);
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);
    let cursor_phys = window.cursor_position().map(|c| c * dpr);

    let target = cursor_phys.and_then(|c| {
        resolve_tmux_inline_wheel_target(&inline, c, cell_w_phys, cell_h_phys, dpr)
    });

    let Some(delta_cells) =
        aggregate_tmux_wheel_cells(&mut wheel, target, inline.browsers.as_deref(), cell_h_logical)
    else {
        // Either the frame was empty or every event was forwarded inline.
        if target.is_some() {
            accumulator.residual_cells = 0.0;
        }
        return;
    };
    // NOTE: a background scroll must not mutate tmux; reset residual on every
    // guarded return so momentum can't lurch on resume. (focused_webview is NOT
    // a guard here — the pointer-gated target above handles webview scrolling.)
    if !window.focused || picker.open || copy_prompt.open.is_some() {
        accumulator.residual_cells = 0.0;
        return;
    }
```

Delete the old `aggregate_wheel_cells(&mut wheel, ...)` call and the old guard line that included `focused_webview.0.is_some()`. Remove the now-unused `focused_webview: Res<FocusedWebview>` parameter from `forward_wheel_to_tmux` (it moved into `TmuxInlineWheelParams`). Keep the rest of the body (active-pane resolution, `consume_wheel_notches`, the notch loop) unchanged.

Add the inline-forking aggregate (mirror native `aggregate_wheel_delta`):

```rust
/// Drains the frame's `MouseWheel` into a signed cell-delta for the tmux path,
/// forking inline-routed events to CEF first. Returns `None` when no
/// terminal-bound events arrived (all forwarded inline, or empty). Identical
/// shape to native `aggregate_wheel_delta`.
fn aggregate_tmux_wheel_cells(
    wheel: &mut MessageReader<MouseWheel>,
    target: Option<TmuxInlineWheelTarget>,
    browsers: Option<&Browsers>,
    cell_h_logical: f32,
) -> Option<f32> {
    let mut delta = 0.0f32;
    let mut had_terminal_input = false;
    for ev in wheel.read() {
        if let Some(target) = target {
            if let Some(browsers) = browsers {
                browsers.send_mouse_wheel(
                    &target.child,
                    target.position_dip,
                    tmux_inline_wheel_delta(ev.unit, ev.x, ev.y),
                );
            }
            continue;
        }
        had_terminal_input = true;
        delta += match ev.unit {
            MouseScrollUnit::Line => ev.y,
            MouseScrollUnit::Pixel => ev.y / cell_h_logical,
        };
    }
    had_terminal_input.then_some(delta)
}
```

> NOTE: keep the existing `aggregate_wheel_cells` only if some other caller uses it; otherwise replace it with `aggregate_tmux_wheel_cells`. Grep for `aggregate_wheel_cells` — if `forward_wheel_to_tmux` was its sole caller, delete it and update its tests to call `aggregate_tmux_wheel_cells` with `target = None`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --bin ozmux-gui wheel_target_ -- --test-threads=1`
Expected: PASS.
Run the existing wheel tests: `cargo test --bin ozmux-gui scroll_command consume_wheel aggregate -- --test-threads=1`
Expected: PASS (update any `aggregate_wheel_cells` test to the new name/signature with `target = None`).

- [ ] **Step 6: Commit**

```bash
git add src/tmux_input.rs src/tmux_mouse.rs
git commit -m "feat(tmux): pointer-gate the wheel so a focused inline webview scrolls instead of copy mode"
```

---

## Task 5: build, lint, full test, manual smoke

**Files:** none (verification only).

- [ ] **Step 1: Full workspace build**

Run: `cargo build`
Expected: success, no errors.

- [ ] **Step 2: Clippy + fmt**

Run: `cargo clippy --workspace --all-targets 2>&1 | tail -40 && cargo fmt --check`
Expected: no warnings in the changed files; fmt clean. Fix any `#[allow]` that should be `#[expect(..., reason = "...")]` per the repo rules.

- [ ] **Step 3: Full test suite (single-threaded to avoid the known parallel-teardown SIGSEGV)**

Run: `cargo test --bin ozmux-gui -- --test-threads=1`
Expected: all green (modulo the pre-existing IME-test caveat noted in repo memory; if that one fails, it is unrelated — confirm it fails on `main` too).

- [ ] **Step 4: Manual smoke (requires CEF; document the result)**

Run `make setup-cef` once if needed, then `cargo run` under a tmux session that mounts an inline webview (e.g. the `ozmd` markdown viewer). Verify, and note pass/fail in the commit or PR body:
1. Wheel over the webview rect scrolls the **webview** (not copy mode).
2. Wheel over the terminal region of the same pane still enters/scrolls **tmux copy mode**.
3. A single click on the webview focuses it AND activates the clicked element (link/button) — the first click is not swallowed.
4. Clicking the terminal region releases webview focus and `select-pane`s.
5. `Ctrl+Shift+Esc` releases focus; keyboard returns to tmux.
6. Hover styling and in-rect text selection (press-drag) work inside the webview.

- [ ] **Step 5: Commit any lint/fmt fixups**

```bash
git add -A
git commit -m "chore(tmux): clippy/fmt cleanup for inline-webview mouse routing"
```

---

## Self-review notes (already reconciled against the spec)

- Spec §"State & focus lifecycle" → Tasks 1 (validator) + 2 (`inline_press`, ungated `set_focus` before click).
- Spec §"Data flow / Click" → Task 2 (`route_tmux_inline_left_click`, modal-guard split-drop).
- Spec §"Data flow / Move" → Task 3 (`forward_tmux_inline_mouse_moves`, stateless, held buttons).
- Spec §"Data flow / Wheel" → Task 4 (`resolve_tmux_inline_wheel_target`, `aggregate_tmux_wheel_cells`, blanket early-return removed).
- Spec §"Keyboard — unchanged" → no task (verified correct; `forward_keys_to_tmux` works once `FocusedWebview` is tmux-aware via Task 1).
- Spec §"Testing" → tests embedded in Tasks 1-4 + Task 5 manual smoke.
- Type consistency: `inline_press: Option<Entity>`, `tmux_pane_local_at(...) -> Option<(Entity, Vec2)>`, `TmuxInlineWheelTarget { child, position_dip }`, `tmux_inline_wheel_delta`, `aggregate_tmux_wheel_cells(..., target, browsers, cell_h)` are used consistently across tasks. `tmux_pane_local_at` is `pub(crate)` (Task 2) so Task 4 imports it.
