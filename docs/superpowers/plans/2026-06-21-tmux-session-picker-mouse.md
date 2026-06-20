# Session Picker Mouse Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add click-to-open, hover-to-highlight, and wheel-scrolling (with a height-capped scroll area) to the keyboard-only tmux session picker.

**Architecture:** All work is in `src/picker.rs`. Rows gain Bevy's `Button` (which carries `Interaction`) plus a `PickerRowLabel(usize)` index. One merged system (`handle_picker_row_interaction`) routes `Pressed`→open and `Hovered`→highlight, sharing an extracted `activate_row` helper with the keyboard `Enter` path. The list becomes a height-capped scroll container; a wheel system and a keep-selected-visible system drive its `ScrollPosition`. Two pure helpers (`wheel_delta_px`, `reveal_offset`) carry the unit-testable math.

**Tech Stack:** Rust (edition 2024), Bevy 0.18.1 ECS + `bevy_ui` (`Button`/`Interaction`, `ScrollPosition`, `Overflow::scroll_y`, `ComputedNode`, `UiGlobalTransform`, `UiSystems`), `MouseWheel`/`MouseScrollUnit`.

## Global Constraints

- Bevy is pinned to **0.18.1**; edition **2024**; toolchain **1.95**. (Verbatim from `Cargo.lock`.)
- All changes live in `src/picker.rs` (plus its `#[cfg(test)] mod tests`). No new modules; no changes outside this file.
- `.claude/rules/rust.md` applies: no `mod.rs`; comments only `// TODO:`/`// NOTE:`/`// SAFETY:` (NOTE = critical caveat only); doc-comment every `pub` item; all `use` at the top in one contiguous block, no inline fully-qualified paths; **mutable params before immutable**; gate whole-system change checks with `run_if`, never in-body early return; let `DerefMut` drive change detection (guard writes with an equality check — no manual `set_changed`); `Plugin::build` is one method chain; `Query` params use descriptive nouns, never a `_q` suffix; private-items-last ordering within a block; minimize visibility (these are all module-internal items → no visibility modifier).
- All in-code comments in English.
- Reference facts (already verified against Bevy 0.18.1 source):
  - `ScrollPosition(pub Vec2)` — values in **logical px**; access `scroll.0.y`. The layout system clamps only the *rendered* `ComputedNode.scroll_position`, **not** this component, so wheel/reveal code must clamp `scroll.0.y` itself.
  - `ComputedNode::size() -> Vec2` and `ComputedNode::content_size() -> Vec2` are **physical px**; `ComputedNode.inverse_scale_factor: f32` converts physical→logical (`logical = physical * inverse_scale_factor`).
  - `UiGlobalTransform` derefs to `Affine2`; `.translation` is the node center in **physical px**.
  - `Button` requires (auto-inserts) `Node`, `FocusPolicy`, `Interaction`.

## File Structure

- **Modify:** `src/picker.rs` — the only production file. New items (all module-private):
  - Component change: `PickerRowLabel` becomes a tuple struct `PickerRowLabel(usize)`.
  - Helper fn: `activate_row(..)` (extracted from the `Enter` arm).
  - Run condition: `picker_is_open`.
  - Systems: `handle_picker_row_interaction`, `picker_row_hover_cursor`, `handle_picker_scroll`, `scroll_selected_into_view`.
  - Pure helpers + const: `wheel_delta_px`, `reveal_offset`, `LINE_SCROLL_PX`.
  - Layout edits in `spawn_picker_ui` (panel `max_height`, list `overflow`/`flex_grow`/`min_height`/`ScrollPosition`) and `sync_picker_ui` (rows spawn with `Button` + `PickerRowLabel(i)`).
- **Modify (tests):** `src/picker.rs` `mod tests` — add unit tests for the pure helpers and an App test for hover.

---

### Task 1: Interactive rows + scroll-container layout

Make rows carry `Button` + their index, and turn the list into a height-capped vertical scroll area. No new behavior yet (nothing reads the index or scrolls), so this is a self-contained structural change verified by build + a new spawn test.

**Files:**
- Modify: `src/picker.rs` (imports; `PickerRowLabel`; `spawn_picker_ui`; `sync_picker_ui`)
- Test: `src/picker.rs` `mod tests`

**Interfaces:**
- Produces: `struct PickerRowLabel(usize)` — a module-private marker component carrying the row's linear position in `build_rows` order. Each picker row entity also carries `Button`. The `PickerList` node carries `ScrollPosition` and uses `Overflow::scroll_y()`.

- [ ] **Step 1: Add the new imports**

Add these three lines into the existing top-of-file `use` block (immediately after the existing `use bevy::prelude::*;` line — keep the block contiguous, no blank lines between imports):

```rust
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::ui::{ComputedNode, ScrollPosition, UiGlobalTransform, UiSystems};
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
```

(Some of these names are also in `bevy::prelude`; an explicit `use` of the same item alongside the glob is allowed and is required by the "no inline fully-qualified paths" rule. `MouseScrollUnit`, `MouseWheel`, `UiGlobalTransform`, `UiSystems`, and the `bevy::window::*` cursor types are *not* in the prelude, so they must be imported.)

- [ ] **Step 2: Make `PickerRowLabel` carry the row index**

Replace the unit marker:

```rust
#[derive(Component)]
struct PickerRowLabel;
```

with a tuple struct holding the row's linear index:

```rust
#[derive(Component)]
struct PickerRowLabel(usize);
```

- [ ] **Step 3: Add `max_height` to the panel and scroll config + `ScrollPosition` to the list**

In `spawn_picker_ui`, the inner panel `Node` (the `.with_children` child with `min_width: Val::Px(360.0)`) gains a `max_height`. Change its `Node { .. }` to include:

```rust
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        min_width: Val::Px(360.0),
                        max_height: Val::Vh(65.0),
                        padding: UiRect::axes(Val::Px(20.0), Val::Px(16.0)),
                        row_gap: Val::Px(10.0),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(8.0)),
                        ..default()
                    },
```

Then change the `PickerList` node spawn so the list is a scroll viewport. Replace:

```rust
                    panel.spawn((
                        Node {
                            flex_direction: FlexDirection::Column,
                            width: Val::Percent(100.0),
                            row_gap: Val::Px(2.0),
                            ..default()
                        },
                        PickerList,
                    ));
```

with:

```rust
                    panel.spawn((
                        Node {
                            flex_direction: FlexDirection::Column,
                            width: Val::Percent(100.0),
                            row_gap: Val::Px(2.0),
                            flex_grow: 1.0,
                            min_height: Val::Px(0.0),
                            overflow: Overflow::scroll_y(),
                            ..default()
                        },
                        ScrollPosition::default(),
                        PickerList,
                    ));
```

(`min_height: 0` is the flexbox requirement that lets the list shrink below its content height inside the capped panel and actually scroll.)

- [ ] **Step 4: Spawn rows with `Button` + their index**

In `sync_picker_ui`, the despawn+respawn branch currently does `for (label, text_color, bar_color) in visuals`. Change it to enumerate and attach `Button` + `PickerRowLabel(i)`. Replace:

```rust
        commands.entity(list_entity).with_children(|parent| {
            for (label, text_color, bar_color) in visuals {
                parent.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
                        border_radius: BorderRadius::all(Val::Px(4.0)),
                        ..default()
                    },
                    Text::new(label),
                    TextColor(text_color),
                    BackgroundColor(bar_color),
                    TextFont {
                        font_size: 14.0,
                        ..default()
                    },
                    PickerRowLabel,
                ));
            }
        });
```

with:

```rust
        commands.entity(list_entity).with_children(|parent| {
            for (i, (label, text_color, bar_color)) in visuals.into_iter().enumerate() {
                parent.spawn((
                    Button,
                    Node {
                        width: Val::Percent(100.0),
                        padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
                        border_radius: BorderRadius::all(Val::Px(4.0)),
                        ..default()
                    },
                    Text::new(label),
                    TextColor(text_color),
                    BackgroundColor(bar_color),
                    TextFont {
                        font_size: 14.0,
                        ..default()
                    },
                    PickerRowLabel(i),
                ));
            }
        });
```

The in-place reuse branch (the `if existing.len() == visuals.len()` arm) is unchanged: it queries `With<PickerRowLabel>` and updates text/color/bg only. Row positions are stable across reuse, so the stored index stays correct.

- [ ] **Step 5: Write the failing spawn test**

Add to `mod tests`:

```rust
    #[test]
    fn rows_spawn_as_buttons_carrying_their_index() {
        let mut app = App::new();
        app.insert_resource(SessionPicker {
            sessions: vec![fake_session(0, "alpha")],
            windows: vec![],
            selected: 0,
            open: true,
            last_open: true,
        });
        app.add_systems(Startup, spawn_picker_ui);
        app.add_systems(Update, sync_picker_ui);
        app.update();

        let mut q = app
            .world_mut()
            .query_filtered::<(&PickerRowLabel, Option<&Button>), With<PickerRowLabel>>();
        let mut indices: Vec<usize> = Vec::new();
        for (label, button) in q.iter(app.world()) {
            assert!(button.is_some(), "every picker row must carry Button");
            indices.push(label.0);
        }
        indices.sort_unstable();
        // build_rows([alpha], []) == [Session(0), NewSession] -> indices 0,1
        assert_eq!(indices, vec![0, 1]);
    }
```

- [ ] **Step 6: Run the test to verify it fails**

Run: `cargo test -p ozmux-gui rows_spawn_as_buttons_carrying_their_index`
Expected: FAIL — before Steps 2–4 are applied it would fail to compile (`PickerRowLabel(i)`) or assert; after applying Steps 2–4 it should pass. (If you wrote the test before the edits, it fails to compile against the unit `PickerRowLabel`.)

- [ ] **Step 7: Run the full picker test module to verify everything passes**

Run: `cargo test -p ozmux-gui picker` then `cargo build`
Expected: PASS — `rows_spawn_as_buttons_carrying_their_index`, the existing `nav_reuses_row_entities_in_place`, `row_visuals_choose_tree_format`, etc. all green; the crate builds.

- [ ] **Step 8: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): rows carry Button + index; list becomes a scroll container"
```

---

### Task 2: Extract `activate_row` helper from the `Enter` arm

Pure refactor: move the switch-vs-attach + mode-transition logic out of `handle_picker_input`'s `Enter` arm into a free fn so the upcoming click path can reuse it. No behavior change.

**Files:**
- Modify: `src/picker.rs` (`handle_picker_input` `Enter` arm; new `activate_row`)

**Interfaces:**
- Produces: `fn activate_row(connection: &mut TmuxConnection, state: &mut ConnectionState, next_mode: &mut NextState<AppMode>, configs: &OzmuxConfigsResource, control: Option<&ControlPlaneHandle>, picker: &SessionPicker, current_mode: &AppMode, row: PickerRow)` — applies one picker row: `apply_switch` while attached, else `apply_attach` and (if `should_enter_ozmux`) `next_mode.set(AppMode::Ozmux)`. Does **not** touch `picker.open` (the caller does).

- [ ] **Step 1: Add the `activate_row` helper**

Add this fn (place it directly above `apply_switch`, so private helpers stay grouped and below the systems that expose behavior):

```rust
// NOTE: mirrors the keyboard Enter arm exactly — used by both the Enter handler
// and the mouse click handler so the two paths cannot diverge. Leaves
// `picker.open` to the caller.
fn activate_row(
    connection: &mut TmuxConnection,
    state: &mut ConnectionState,
    next_mode: &mut NextState<AppMode>,
    configs: &OzmuxConfigsResource,
    control: Option<&ControlPlaneHandle>,
    picker: &SessionPicker,
    current_mode: &AppMode,
    row: PickerRow,
) {
    if connection.client().is_some() {
        apply_switch(connection, state, configs, control, picker, row);
    } else {
        let attached = apply_attach(connection, state, configs, control, picker, row);
        if should_enter_ozmux(attached, current_mode) {
            next_mode.set(AppMode::Ozmux);
        }
    }
}
```

- [ ] **Step 2: Rewrite the `Enter` arm to call it**

In `handle_picker_input`, replace the entire `KeyCode::Enter => { .. }` arm with:

```rust
            KeyCode::Enter => {
                let row = rows
                    .get(picker.selected)
                    .copied()
                    .unwrap_or(PickerRow::NewSession);
                activate_row(
                    &mut connection,
                    &mut state,
                    &mut next_mode,
                    &configs,
                    control.as_deref(),
                    &picker,
                    current_mode.get(),
                    row,
                );
                picker.open = false;
                break;
            }
```

- [ ] **Step 3: Verify build + existing tests still pass**

Run: `cargo test -p ozmux-gui picker`
Expected: PASS — `esc_closes_the_picker`, `j_k_move_selection_like_arrows`, `should_enter_ozmux_only_when_attached_from_ozma`, etc. unchanged and green. (The `Enter`→attach path is still not unit-tested — it shells out to real tmux, exactly as before.)

- [ ] **Step 4: Commit**

```bash
git add src/picker.rs
git commit -m "refactor(picker): extract activate_row from the Enter arm"
```

---

### Task 3: Merged click + hover system (`handle_picker_row_interaction`)

One system handles both `Pressed`→open and `Hovered`→highlight (identical query → single system per rust.md). Hover-selection is gated on the user having moved the mouse since the picker opened, so opening under a stationary cursor keeps the keyboard's first-session selection.

**Files:**
- Modify: `src/picker.rs` (new `picker_is_open`, new `handle_picker_row_interaction`, plugin registration)
- Test: `src/picker.rs` `mod tests`

**Interfaces:**
- Consumes: `activate_row(..)` (Task 2); `PickerRowLabel(usize)`, `build_rows`, `PickerRow` (existing).
- Produces: `fn picker_is_open(picker: Res<SessionPicker>) -> bool`; `fn handle_picker_row_interaction(..)`.

- [ ] **Step 1: Add the `picker_is_open` run condition**

Add near the other free fns (below the systems, above the pure helpers):

```rust
fn picker_is_open(picker: Res<SessionPicker>) -> bool {
    picker.open
}
```

- [ ] **Step 2: Add the merged interaction system**

Add this system (place it after `handle_picker_input`, before `apply_switch`):

```rust
// NOTE: `hover_armed` stays false until a CursorMoved arrives after the picker
// opens, so opening under a stationary cursor does not hijack the keyboard's
// first-session selection. Clicks (Pressed) are never gated by it.
fn handle_picker_row_interaction(
    mut picker: ResMut<SessionPicker>,
    mut connection: NonSendMut<TmuxConnection>,
    mut state: ResMut<ConnectionState>,
    mut next_mode: ResMut<NextState<AppMode>>,
    mut cursor_moved: MessageReader<CursorMoved>,
    mut hover_armed: Local<bool>,
    mut was_open: Local<bool>,
    rows: Query<(&Interaction, &PickerRowLabel), Changed<Interaction>>,
    configs: Res<OzmuxConfigsResource>,
    current_mode: Res<State<AppMode>>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    let opened = picker.open && !*was_open;
    *was_open = picker.open;
    if opened {
        *hover_armed = false;
    }
    if cursor_moved.read().count() > 0 {
        *hover_armed = true;
    }

    for (interaction, label) in rows.iter() {
        match interaction {
            Interaction::Pressed => {
                picker.selected = label.0;
                let built = build_rows(&picker.sessions, &picker.windows);
                let row = built
                    .get(picker.selected)
                    .copied()
                    .unwrap_or(PickerRow::NewSession);
                activate_row(
                    &mut connection,
                    &mut state,
                    &mut next_mode,
                    &configs,
                    control.as_deref(),
                    &picker,
                    current_mode.get(),
                    row,
                );
                picker.open = false;
                break;
            }
            Interaction::Hovered => {
                if *hover_armed {
                    picker.selected = label.0;
                }
            }
            Interaction::None => {}
        }
    }
}
```

- [ ] **Step 3: Register the system, gated on `picker_is_open`**

In `OzmuxPickerPlugin::build`, extend the existing `app` method chain (do not start a new `app.` statement) by adding this `.add_systems(..)` call to the chain:

```rust
            .add_systems(
                Update,
                handle_picker_row_interaction.run_if(picker_is_open),
            )
```

- [ ] **Step 4: Write the failing hover App test**

Add to `mod tests`. It reuses the `picker_input_app` resource setup but registers the new system, drives a `CursorMoved` to arm hover, then sets a row's `Interaction` to `Hovered`:

```rust
    fn cursor_moved() -> CursorMoved {
        CursorMoved {
            window: Entity::PLACEHOLDER,
            position: Vec2::ZERO,
            delta: None,
        }
    }

    #[test]
    fn hover_moves_selection_only_after_the_mouse_moves() {
        let mut app = App::new();
        app.add_plugins(StatesPlugin);
        app.insert_state(AppMode::Ozma);
        app.add_message::<CursorMoved>();
        app.insert_resource(SessionPicker {
            sessions: vec![fake_session(0, "alpha"), fake_session(1, "beta")],
            windows: vec![],
            selected: 0,
            open: true,
            last_open: true,
        });
        app.init_resource::<ConnectionState>();
        app.init_resource::<OzmuxConfigsResource>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_systems(Update, handle_picker_row_interaction);

        // A hovered row exists but the mouse has not moved since open: no change.
        let row = app.world_mut().spawn((Interaction::Hovered, PickerRowLabel(1))).id();
        app.update();
        assert_eq!(
            app.world().resource::<SessionPicker>().selected,
            0,
            "stationary-cursor hover must not move selection"
        );

        // Arm by moving the mouse, then re-trigger the hover change.
        app.world_mut().write_message(cursor_moved());
        app.world_mut()
            .entity_mut(row)
            .insert(Interaction::None);
        app.update();
        app.world_mut()
            .entity_mut(row)
            .insert(Interaction::Hovered);
        app.update();
        assert_eq!(
            app.world().resource::<SessionPicker>().selected,
            1,
            "after the mouse moves, hover moves selection"
        );
    }
```

- [ ] **Step 5: Run the test to verify it fails, then passes**

Run: `cargo test -p ozmux-gui hover_moves_selection_only_after_the_mouse_moves`
Expected: before Step 2/3, FAIL to compile (no `handle_picker_row_interaction`); after, PASS.

- [ ] **Step 6: Verify the whole picker module + build**

Run: `cargo test -p ozmux-gui picker && cargo build`
Expected: PASS / builds clean.

- [ ] **Step 7: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): click-to-open + hover-to-highlight via merged interaction system"
```

---

### Task 4: Pointer cursor on row hover (`picker_row_hover_cursor`)

While the picker is open, show a pointer cursor over rows so they read as clickable; revert to the default cursor when not over a row. Authoritative while open (the picker can appear at boot before any baseline cursor system runs).

**Files:**
- Modify: `src/picker.rs` (new `picker_row_hover_cursor`, plugin registration)

**Interfaces:**
- Consumes: `picker_is_open`, `PickerRowLabel` (existing).
- Produces: `fn picker_row_hover_cursor(..)`.

- [ ] **Step 1: Add the cursor system**

Add after `handle_picker_row_interaction`:

```rust
fn picker_row_hover_cursor(
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    rows: Query<&Interaction, With<PickerRowLabel>>,
) {
    let hovering = rows
        .iter()
        .any(|i| matches!(i, Interaction::Hovered | Interaction::Pressed));
    let Ok(mut icon) = cursor_icons.single_mut() else {
        return;
    };
    let is_pointer = matches!(&*icon, CursorIcon::System(e) if *e == SystemCursorIcon::Pointer);
    if hovering && !is_pointer {
        *icon = CursorIcon::System(SystemCursorIcon::Pointer);
    } else if !hovering && is_pointer {
        *icon = CursorIcon::System(SystemCursorIcon::Default);
    }
}
```

(The `is_pointer` guard keeps the write conditional so `Changed<CursorIcon>` stays honest — no per-frame churn.)

- [ ] **Step 2: Register it after `InputPhase::Hover`, gated on `picker_is_open`**

Add to the `OzmuxPickerPlugin::build` chain:

```rust
            .add_systems(
                Update,
                picker_row_hover_cursor
                    .after(crate::input::InputPhase::Hover)
                    .run_if(picker_is_open),
            )
```

- [ ] **Step 3: Verify build + tests**

Run: `cargo build && cargo test -p ozmux-gui picker`
Expected: builds clean; picker tests green. (Cursor behavior is verified manually in Task 7 — there is no headless cursor assertion.)

- [ ] **Step 4: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): pointer cursor while hovering a row"
```

---

### Task 5: Wheel scrolling (`wheel_delta_px` + `handle_picker_scroll`)

Convert wheel events to a clamped `ScrollPosition` write on the list. The pure `wheel_delta_px` is TDD'd first; the system aggregates the frame's events and clamps the logical offset itself (layout does not clamp the component).

**Files:**
- Modify: `src/picker.rs` (new `LINE_SCROLL_PX`, `wheel_delta_px`, `handle_picker_scroll`, plugin registration)
- Test: `src/picker.rs` `mod tests`

**Interfaces:**
- Consumes: `picker_is_open`, `PickerList` (existing).
- Produces: `const LINE_SCROLL_PX: f32`; `fn wheel_delta_px(unit: MouseScrollUnit, y: f32) -> f32` (returns a **logical-px** delta; sign matches `ScrollPosition` — wheel-down yields a positive delta); `fn handle_picker_scroll(..)`.

- [ ] **Step 1: Write the failing `wheel_delta_px` tests**

Add to `mod tests`:

```rust
    #[test]
    fn wheel_line_delta_is_inverted_and_scaled_by_row_stride() {
        // Wheel up (y>0) scrolls content toward the top -> negative offset delta.
        assert_eq!(wheel_delta_px(MouseScrollUnit::Line, 1.0), -LINE_SCROLL_PX);
        // Wheel down (y<0) -> positive offset delta.
        assert_eq!(wheel_delta_px(MouseScrollUnit::Line, -2.0), 2.0 * LINE_SCROLL_PX);
    }

    #[test]
    fn wheel_pixel_delta_is_inverted_identity() {
        assert_eq!(wheel_delta_px(MouseScrollUnit::Pixel, 5.0), -5.0);
        assert_eq!(wheel_delta_px(MouseScrollUnit::Pixel, -3.0), 3.0);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ozmux-gui wheel_`
Expected: FAIL to compile — `wheel_delta_px` / `LINE_SCROLL_PX` undefined.

- [ ] **Step 3: Implement `wheel_delta_px` + the constant**

Add near the other pure helpers (bottom of the file, above `mod tests`):

```rust
/// Logical pixels scrolled per wheel "line" notch. Roughly one row stride
/// (row height ≈ 18px + 2px gap).
const LINE_SCROLL_PX: f32 = 24.0;

/// The logical-pixel `ScrollPosition` delta for one wheel event. The sign is
/// inverted relative to the wheel `y` so that wheel-down (negative `y`) yields a
/// positive delta — a larger `ScrollPosition.0.y` moves the content up, i.e. the
/// viewport down. (Same structure as `tmux_inline_wheel_delta` but the opposite
/// sign, because that path drives a terminal, not a scroll offset.)
fn wheel_delta_px(unit: MouseScrollUnit, y: f32) -> f32 {
    match unit {
        MouseScrollUnit::Line => -y * LINE_SCROLL_PX,
        MouseScrollUnit::Pixel => -y,
    }
}
```

- [ ] **Step 4: Run to verify the helper tests pass**

Run: `cargo test -p ozmux-gui wheel_`
Expected: PASS.

- [ ] **Step 5: Add the wheel system**

Add after `picker_row_hover_cursor`:

```rust
fn handle_picker_scroll(
    mut list: Query<(&mut ScrollPosition, &ComputedNode), With<PickerList>>,
    mut wheel: MessageReader<MouseWheel>,
) {
    let Ok((mut pos, node)) = list.single_mut() else {
        wheel.clear();
        return;
    };
    let mut delta = 0.0;
    for ev in wheel.read() {
        delta += wheel_delta_px(ev.unit, ev.y);
    }
    if delta == 0.0 {
        return;
    }
    let inv = node.inverse_scale_factor;
    let max = (node.content_size().y - node.size().y).max(0.0) * inv;
    let next = (pos.0.y + delta).clamp(0.0, max);
    if pos.0.y != next {
        pos.0.y = next;
    }
}
```

- [ ] **Step 6: Register it, gated on `MouseWheel` + `picker_is_open`**

Add to the `OzmuxPickerPlugin::build` chain:

```rust
            .add_systems(
                Update,
                handle_picker_scroll
                    .run_if(on_message::<MouseWheel>)
                    .run_if(picker_is_open),
            )
```

- [ ] **Step 7: Verify build + tests**

Run: `cargo build && cargo test -p ozmux-gui picker`
Expected: builds clean; all picker tests (including the new wheel ones) green.

- [ ] **Step 8: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): wheel-scroll the session list"
```

---

### Task 6: Keep the selected row visible (`reveal_offset` + `scroll_selected_into_view`)

Once the list scrolls, keyboard navigation must keep the selected row on screen. The pure `reveal_offset` is TDD'd first; the system measures the selected row's real laid-out rectangle (no uniform-height assumption), converts physical→logical, and writes a clamped `ScrollPosition`.

**Files:**
- Modify: `src/picker.rs` (new `reveal_offset`, `scroll_selected_into_view`, plugin registration + `sync_picker_ui` ordering)
- Test: `src/picker.rs` `mod tests`

**Interfaces:**
- Consumes: `picker_is_open`, `PickerList`, `PickerRowLabel`, `SessionPicker` (existing).
- Produces: `fn reveal_offset(row_top: f32, row_h: f32, viewport_h: f32, current: f32, max: f32) -> f32` (all args logical px; content-relative `row_top`; returns the new offset clamped to `[0, max]`); `fn scroll_selected_into_view(..)`.

- [ ] **Step 1: Write the failing `reveal_offset` tests**

Add to `mod tests`:

```rust
    #[test]
    fn reveal_leaves_a_fully_visible_row_unchanged() {
        // row [40,60] inside viewport [30,130]: unchanged.
        assert_eq!(reveal_offset(40.0, 20.0, 100.0, 30.0, 200.0), 30.0);
    }

    #[test]
    fn reveal_scrolls_up_so_row_top_is_flush() {
        // row top 10 is above current 50: offset becomes 10.
        assert_eq!(reveal_offset(10.0, 20.0, 100.0, 50.0, 200.0), 10.0);
    }

    #[test]
    fn reveal_scrolls_down_so_row_bottom_is_flush() {
        // row [180,200], viewport height 100, current 0: 200 - 100 = 100.
        assert_eq!(reveal_offset(180.0, 20.0, 100.0, 0.0, 200.0), 100.0);
    }

    #[test]
    fn reveal_clamps_to_zero_and_to_max() {
        assert_eq!(reveal_offset(-10.0, 20.0, 100.0, 5.0, 200.0), 0.0);
        assert_eq!(reveal_offset(180.0, 20.0, 100.0, 0.0, 50.0), 50.0);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ozmux-gui reveal_`
Expected: FAIL to compile — `reveal_offset` undefined.

- [ ] **Step 3: Implement `reveal_offset`**

Add near the other pure helpers:

```rust
/// The new vertical scroll offset (logical px) that brings the row spanning
/// `[row_top, row_top + row_h]` fully into a `viewport_h`-tall viewport currently
/// scrolled to `current`. Scrolls up if the row is above the viewport, down if
/// below, else unchanged; the result is clamped to `[0, max]`.
fn reveal_offset(row_top: f32, row_h: f32, viewport_h: f32, current: f32, max: f32) -> f32 {
    let target = if row_top < current {
        row_top
    } else if row_top + row_h > current + viewport_h {
        row_top + row_h - viewport_h
    } else {
        current
    };
    target.clamp(0.0, max.max(0.0))
}
```

- [ ] **Step 4: Run to verify the helper tests pass**

Run: `cargo test -p ozmux-gui reveal_`
Expected: PASS.

- [ ] **Step 5: Add the scroll-into-view system**

Add after `handle_picker_scroll`:

```rust
// NOTE: writes ScrollPosition after UiSystems::Layout, so the correction lands on
// the next frame's render. Reads physical-px geometry (ComputedNode size +
// UiGlobalTransform center) and converts to logical via inverse_scale_factor,
// because ScrollPosition is in logical px.
fn scroll_selected_into_view(
    mut list: Query<(&mut ScrollPosition, &ComputedNode, &UiGlobalTransform, &Children), With<PickerList>>,
    rows: Query<(&ComputedNode, &UiGlobalTransform, &PickerRowLabel)>,
    picker: Res<SessionPicker>,
) {
    let Ok((mut pos, list_node, list_tf, children)) = list.single_mut() else {
        return;
    };
    let viewport_h_phys = list_node.size().y;
    if viewport_h_phys <= 0.0 {
        return;
    }

    let mut selected: Option<(f32, f32)> = None;
    for &child in children.iter() {
        let Ok((row_node, row_tf, label)) = rows.get(child) else {
            continue;
        };
        if label.0 == picker.selected {
            let h = row_node.size().y;
            let top = row_tf.translation.y - h / 2.0;
            selected = Some((top, h));
            break;
        }
    }
    let Some((row_top_global, row_h_phys)) = selected else {
        return;
    };
    if row_h_phys <= 0.0 {
        return;
    }

    let inv = list_node.inverse_scale_factor;
    let list_top_global = list_tf.translation.y - viewport_h_phys / 2.0;
    let current = pos.0.y;
    let row_top = current + (row_top_global - list_top_global) * inv;
    let row_h = row_h_phys * inv;
    let viewport_h = viewport_h_phys * inv;
    let max = (list_node.content_size().y - viewport_h_phys).max(0.0) * inv;

    let next = reveal_offset(row_top, row_h, viewport_h, current, max);
    if pos.0.y != next {
        pos.0.y = next;
    }
}
```

- [ ] **Step 6: Register it after layout + add the `sync_picker_ui` ordering edge**

In `OzmuxPickerPlugin::build`, change the existing `sync_picker_ui` registration so it runs before layout (so freshly-spawned rows have same-frame sizes). Replace:

```rust
            .add_systems(
                PostUpdate,
                sync_picker_ui.run_if(resource_exists_and_changed::<SessionPicker>),
            );
```

with (note: this is the chain's terminal call today — keep the trailing `;` on the new last call):

```rust
            .add_systems(
                PostUpdate,
                sync_picker_ui
                    .before(UiSystems::Layout)
                    .run_if(resource_exists_and_changed::<SessionPicker>),
            )
            .add_systems(
                PostUpdate,
                scroll_selected_into_view
                    .after(UiSystems::Layout)
                    .run_if(picker_is_open)
                    .run_if(resource_exists_and_changed::<SessionPicker>),
            );
```

- [ ] **Step 7: Verify build + tests**

Run: `cargo build && cargo test -p ozmux-gui picker`
Expected: builds clean; all picker tests green (the existing `nav_reuses_row_entities_in_place` test registers `sync_picker_ui` in `Update` without UI plugins, so the `.before(UiSystems::Layout)` edge in the plugin does not affect it).

- [ ] **Step 8: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): keep the selected row in view when scrolling"
```

---

### Task 7: Final verification — lint, format, full test, manual smoke

Confirm the whole change is clean and actually works in the running app.

**Files:**
- Modify: `src/picker.rs` (only if clippy/fmt request changes)

- [ ] **Step 1: Lint + format**

Run: `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`
Expected: no remaining warnings in `src/picker.rs`; fmt leaves the file clean.

- [ ] **Step 2: Full test suite**

Run: `cargo test`
Expected: PASS (workspace-wide; in particular every `picker` test).

- [ ] **Step 3: Manual smoke test**

Run the app against a tmux server that has several sessions (enough rows to overflow ~65% of the window height):

Run: `cargo run`
Verify, in the picker:
- Hovering a row moves the amber highlight to it; the cursor becomes a pointer over rows and reverts off them.
- Opening the picker with the cursor already over the panel leaves the first session highlighted until you move the mouse.
- Single-clicking a session / window / "New session" row opens it (attach/switch/create) exactly like pressing Enter would.
- The mouse wheel scrolls the list; it stops cleanly at the top and bottom (no phantom over-scroll).
- Arrowing past the bottom of the visible area scrolls the selected row back into view.

- [ ] **Step 4: Commit any lint/fmt fixups**

```bash
git add -A
git commit -m "chore(picker): clippy + fmt for mouse support" || echo "nothing to commit"
```

## Self-Review Notes

- **Spec coverage:** click-to-open (Task 3), hover-to-highlight (Task 3), wheel-scroll (Task 5), height-capped scroll area (Task 1), keep-selected-visible (Task 6), shared `activate_row` Approach A (Task 2), pointer cursor (Task 4), the two Bevy-0.18.1 API corrections (`ScrollPosition.0.y` and `UiSystems::Layout` — used throughout Tasks 5/6), self-clamping (Tasks 5/6), measured-geometry reveal (Task 6), merged interaction system (Task 3), open-gate + `sync_picker_ui.before(UiSystems::Layout)` ordering (Task 6). Out-of-scope items (click-outside-dismiss, scrollbar thumb, double-click, digit-jump, footer text) are intentionally absent.
- **Type consistency:** `PickerRowLabel(usize)` (Task 1) is read as `label.0` everywhere (Tasks 3, 6). `activate_row` signature in Task 2 matches its call sites in Tasks 2 and 3. `wheel_delta_px`/`reveal_offset` signatures in Tasks 5/6 match their tests and call sites. `picker_is_open` (Task 3) is reused in Tasks 4/5/6.
- **Known residual risk:** the physical→logical conversion in `scroll_selected_into_view` (Task 6) is the one piece not covered by a pure unit test (the math helper `reveal_offset` is; the measurement/conversion is checked by the Task 7 manual smoke). On a non-HiDPI display `inverse_scale_factor == 1.0`, so the conversion is the identity; the smoke test should be run on the target display to confirm HiDPI behavior.
