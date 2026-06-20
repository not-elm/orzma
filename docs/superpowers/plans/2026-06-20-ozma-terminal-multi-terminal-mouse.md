# ozma_terminal Multi-Terminal Mouse Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `ozma_terminal` crate's mouse handling route to the topmost `OzmaTerminal` under the cursor when several terminals are live and overlapping, with per-terminal gesture/selection state and a per-entity mouse gate.

**Architecture:** Replace `terminal.single()` in the three mouse systems with a `topmost_terminal_at` hit-test over all eligible terminals (highest `ComputedNode::stack_index` wins). The single gesture resource embeds the press target `Entity` so drag/release stay locked to the press terminal. Split the one `InputDisabled` marker into `KeyboardDisabled` (keyboard, still `.single()`) + `MouseDisabled` (mouse, per-entity); modal suppression falls out of an empty candidate set. The pure routers (`decide_button` / `decide_wheel`) are unchanged.

**Tech Stack:** Rust 2024 (toolchain 1.95), Bevy 0.18.1 ECS, `alacritty_terminal` (via the engine), existing `arboard` clipboard.

**Spec:** `docs/superpowers/specs/2026-06-20-ozma-terminal-multi-terminal-mouse-design.md`

## Global Constraints

- Rust edition 2024, toolchain 1.95. Every externally-`pub` item gets a `///` doc; every module file has a `//!` header.
- No `mod.rs`. Comments restricted to `// TODO:` / `// NOTE:` / `// SAFETY:`, all in English. `// NOTE:` only for critical caveats (concrete harm if overlooked).
- Imports: one contiguous `use` block at the top; no inline fully-qualified paths in signatures/bodies/type params.
- Bevy systems: mutable `SystemParam`s declared before immutable ones; gate whole-system change checks with `run_if`, not in-body early returns; `Plugin::build` is one method chain; `Query` params use descriptive nouns (no `_q`).
- Visibility minimized (private unless a cross-module caller forces wider); private items declared after exported ones; prefer `#[expect(reason=…)]` over `#[allow]`.
- The `ozma_terminal` crate MUST NOT depend on `ozmux_configs`, `ozmux_tmux`, `bevy_cef`, or host types (`NonInteractive` is a host marker; `MouseDisabled` is the crate-local equivalent and stays separate).
- Behavior MUST be preserved for the single-terminal case (the only case the current host produces).

## File Structure

| File | Responsibility | Change |
| --- | --- | --- |
| `crates/ozma_terminal/src/input.rs` | Keyboard dispatcher + keyboard gate marker | Rename `InputDisabled` → `KeyboardDisabled`; add a guardrail `// NOTE:` at the `.single()` site |
| `crates/ozma_terminal/src/mouse.rs` | Mouse dispatchers, gesture state, hit-test | Add `MouseDisabled` marker + `topmost_terminal_at`; multi-terminal `dispatch_mouse_buttons` / `dispatch_mouse_wheel`; `entity` in `HeldPointer`; `last_target` in `WheelAccumulator` |
| `crates/ozma_terminal/src/hyperlink.rs` | Cmd-click + hover-cursor feedback | Hover hit-tests topmost; filter `Without<MouseDisabled>` |
| `crates/ozma_terminal/src/lib.rs` | Crate plugin + re-exports | Export `KeyboardDisabled` (renamed) + `MouseDisabled` |
| `crates/ozma_terminal/src/spawn.rs` | `OzmaTerminal` marker + spawn | Fix the "Exactly one entity" doc for the multi-terminal premise |
| `src/ozma_input.rs` | Host gate maintainer | `maintain_input_disabled` → `maintain_input_gates`, sync both markers |

---

## Task 1: Split the input gate into `KeyboardDisabled` + `MouseDisabled` (behavior-preserving)

A pure refactor: rename the keyboard marker, add the mouse marker, point each dispatcher at its own marker, and have the host maintain both from the same `disable` bool. The mouse systems still use `.single()` here — multi-terminal routing lands in Tasks 2–4. After this task the single-terminal behavior is byte-identical.

**Files:**
- Modify: `crates/ozma_terminal/src/input.rs`
- Modify: `crates/ozma_terminal/src/mouse.rs`
- Modify: `crates/ozma_terminal/src/hyperlink.rs`
- Modify: `crates/ozma_terminal/src/lib.rs`
- Modify: `crates/ozma_terminal/src/spawn.rs`
- Modify: `src/ozma_input.rs`

**Interfaces:**
- Produces: `KeyboardDisabled` (component, `pub`, in `input.rs`), `MouseDisabled` (component, `pub`, in `mouse.rs`), both re-exported from `lib.rs`.
- Removed: `InputDisabled` (no longer exists anywhere).

- [ ] **Step 1: Rename `InputDisabled` → `KeyboardDisabled` in `input.rs`**

In `crates/ozma_terminal/src/input.rs`:
- Line 4 (`//!` header): change `Gated per\n//! entity by the `InputDisabled` marker.` → `Gated per\n//! entity by the `KeyboardDisabled` marker.`
- Lines 14-18: rename the type and update its doc:

```rust
/// When present on an `OzmaTerminal` entity, the crate's default keyboard
/// dispatcher skips it entirely — the host routes keyboard input elsewhere
/// (tmux, a focused webview, an open picker, IME composition).
#[derive(Component)]
pub struct KeyboardDisabled;
```

- Line 67 (doc on `OzmaTerminalInputSet`): change `maintain\n/// `InputDisabled`` → `maintain\n/// `KeyboardDisabled``.
- Line 93: change the query filter and add the guardrail NOTE just above the `.single()`:

```rust
    terminal: Query<Entity, (With<OzmaTerminal>, Without<KeyboardDisabled>)>,
) {
    // NOTE: keyboard keeps the single-terminal model — a future multi-terminal
    // host MUST keep exactly one OzmaTerminal un-`KeyboardDisabled`, or this
    // `.single()` returns Err and every keypress is silently dropped.
    let Ok(entity) = terminal.single() else {
```

- [ ] **Step 2: Add `MouseDisabled` and switch the mouse filters in `mouse.rs`**

In `crates/ozma_terminal/src/mouse.rs`:
- Line 5 (`//!` header): change `Gated per entity by `InputDisabled`.` → `Gated per entity by `MouseDisabled`.`
- Line 24: drop `InputDisabled` from the import:

```rust
use crate::input::current_terminal_modifiers;
```

- Add the marker next to `OzmaTerminalMouseSet` (after line 80):

```rust
/// When present on an `OzmaTerminal` entity, the crate's mouse dispatchers and
/// hover-cursor system skip it — it is removed from the hit-test candidate set,
/// so the pointer falls through to the next terminal below it. The host marks
/// every terminal `MouseDisabled` for modal suppression (picker / IME / focused
/// webview / unfocused window).
#[derive(Component)]
pub struct MouseDisabled;
```

- Line 78 (doc on `OzmaTerminalMouseSet`): change `maintaining\n/// `InputDisabled`` → `maintaining\n/// `MouseDisabled``.
- Line 358 (doc on `dispatch_mouse_buttons`): change `Skips the\n/// `OzmaTerminal` while it carries `InputDisabled`.` → `Skips any\n/// `OzmaTerminal` carrying `MouseDisabled`.`
- Line 371 and line 486 (both dispatcher query filters): change `(With<OzmaTerminal>, Without<InputDisabled>)` → `(With<OzmaTerminal>, Without<MouseDisabled>)`.

- [ ] **Step 3: Switch hover to `MouseDisabled` in `hyperlink.rs`**

In `crates/ozma_terminal/src/hyperlink.rs`:
- Line 6: change `use crate::input::InputDisabled;` → `use crate::mouse::MouseDisabled;` (keep alphabetical-agnostic placement; the existing `use crate::mouse::{...}` is on line 7, so this becomes a second `crate::mouse` line — merge into one: `use crate::mouse::{MouseDisabled, OzmaTerminalMouseSet, cell_at_cursor, protocol_mods};`).
- Lines 64 and 99 (the two query filters in `hyperlink_hover_cursor` and `resolve_hover`): change `(With<OzmaTerminal>, Without<InputDisabled>)` → `(With<OzmaTerminal>, Without<MouseDisabled>)`.

- [ ] **Step 4: Update re-exports in `lib.rs`**

In `crates/ozma_terminal/src/lib.rs`:
- Line 21: change `pub use input::{InputDisabled, OzmaTerminalInputSet, ReservedChord, TerminalInputBindings};` → `pub use input::{KeyboardDisabled, OzmaTerminalInputSet, ReservedChord, TerminalInputBindings};`
- Line 22: change `pub use mouse::{FineModifier, OzmaMouseConfig, OzmaTerminalMouseSet};` → `pub use mouse::{FineModifier, MouseDisabled, OzmaMouseConfig, OzmaTerminalMouseSet};`

- [ ] **Step 5: Maintain both markers in the host (`src/ozma_input.rs`)**

In `src/ozma_input.rs`:
- Line 1 (`//!`): change `maintains the crate's `InputDisabled`` → `maintains the crate's `KeyboardDisabled` / `MouseDisabled``.
- Line 17: change the import to `use ozma_terminal::{KeyboardDisabled, MouseDisabled, OzmaTerminal, OzmaTerminalInputSet, OzmaTerminalMouseSet};`
- Line 27 (registration): rename the system reference `maintain_input_disabled` → `maintain_input_gates`.
- Replace the whole `fn maintain_input_disabled(...)` (lines 42-64) with:

```rust
fn maintain_input_gates(
    mut commands: Commands,
    picker: Res<SessionPicker>,
    ime: Res<ImeState>,
    focused_webview: Res<FocusedWebview>,
    windows: Query<&Window, With<PrimaryWindow>>,
    terminals: Query<(Entity, Has<KeyboardDisabled>, Has<MouseDisabled>), With<OzmaTerminal>>,
) {
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    let disable = should_disable_input(
        picker.open,
        ime.is_composing(),
        focused,
        focused_webview.0.is_some(),
    );
    for (entity, has_keyboard, has_mouse) in terminals.iter() {
        if disable && !has_keyboard {
            commands.entity(entity).insert(KeyboardDisabled);
        } else if !disable && has_keyboard {
            commands.entity(entity).remove::<KeyboardDisabled>();
        }
        if disable && !has_mouse {
            commands.entity(entity).insert(MouseDisabled);
        } else if !disable && has_mouse {
            commands.entity(entity).remove::<MouseDisabled>();
        }
    }
}
```

- [ ] **Step 6: Update the affected tests and the `OzmaTerminal` doc**

- `crates/ozma_terminal/src/input.rs:269` — in `input_disabled_entity_fires_nothing`, change the spawn to `app.world_mut().spawn((OzmaTerminal, KeyboardDisabled));`. Rename the test to `keyboard_disabled_entity_fires_nothing`.
- `crates/ozma_terminal/src/mouse.rs:805` — in `input_disabled_terminal_drains_without_arming_a_gesture`, change the spawn to `app.world_mut().spawn((OzmaTerminal, MouseDisabled));`. Rename the test to `mouse_disabled_terminal_drains_without_arming_a_gesture`.
- `crates/ozma_terminal/src/spawn.rs:11-14` — replace the `OzmaTerminal` doc with:

```rust
/// Marker component identifying an Ozma-mode terminal entity.
///
/// One or more entities may carry this marker; mouse input routes to the
/// topmost under the cursor, while keyboard input targets the single entity the
/// host leaves un-`KeyboardDisabled`.
#[derive(Component)]
pub struct OzmaTerminal;
```

- [ ] **Step 7: Build, test, lint**

Run: `cargo test -p ozma_terminal && cargo build && cargo clippy -p ozma_terminal -p ozmux-gui`
Expected: PASS; no remaining references to `InputDisabled` (verify with `grep -rn InputDisabled src/ crates/` → no output).

- [ ] **Step 8: Commit**

```bash
git add crates/ozma_terminal/src/{input.rs,mouse.rs,hyperlink.rs,lib.rs,spawn.rs} src/ozma_input.rs
git commit -m "refactor(ozma_terminal): split InputDisabled into KeyboardDisabled + MouseDisabled

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `topmost_terminal_at` helper + multi-terminal `dispatch_mouse_buttons`

Add the z-ordered hit-test (the one piece of new logic that is unit-testable without a `TerminalHandle`), embed the press `Entity` in the gesture, and rewrite the button dispatcher to route per-event: press → topmost terminal; drag/release → the stored press terminal.

**Files:**
- Modify: `crates/ozma_terminal/src/mouse.rs`

**Interfaces:**
- Consumes: `ComputedNode::stack_index()` / `contains_point` (Bevy), existing `CellContext`, `resolve_button_event`, `synthesize_drag`, `decide_button`.
- Produces: `pub(crate) fn topmost_terminal_at<'a>(cursor_phys: Vec2, candidates: impl Iterator<Item = (Entity, &'a ComputedNode, &'a UiGlobalTransform)>) -> Option<Entity>`; `HeldPointer { entity: Entity, button: MouseButtonKind, last_cell: CellCoord }`.

- [ ] **Step 1: Add `entity` to `HeldPointer`**

In `crates/ozma_terminal/src/mouse.rs`, replace the `HeldPointer` struct (lines 108-112) with:

```rust
/// A held mouse button: the terminal the press landed on, the button, and the
/// last cell a drag was synthesized for. The `entity` locks drag/release to the
/// press terminal even when the pointer wanders onto another terminal. Tracked
/// for BOTH local selection and app-forward drags — the forward path never sets
/// `drag`, so drag-motion synthesis must not depend on it.
#[derive(Clone, Copy)]
pub(crate) struct HeldPointer {
    pub(crate) entity: Entity,
    pub(crate) button: MouseButtonKind,
    pub(crate) last_cell: CellCoord,
}
```

Keep the crate compiling: update the existing construction site in `dispatch_mouse_buttons` (~line 442) to set the new field — the `entity` binding from the current `terminal.single()` is in scope there:

```rust
                gesture.held = Some(HeldPointer {
                    entity,
                    button: evt.button,
                    last_cell: evt.cell,
                });
```

Run `cargo build -p ozma_terminal` to confirm the crate still compiles before adding the helper (it is fully rewritten in Step 5).

- [ ] **Step 2: Write the failing `topmost_terminal_at` test**

Add to `mouse.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn topmost_terminal_at_picks_highest_stack_index_among_containing() {
    let mut world = World::new();
    let a = world.spawn_empty().id();
    let b = world.spawn_empty().id();
    let c = world.spawn_empty().id();
    // A: left half (x 0..400), stack 5. B: right half (x 400..800), stack 3.
    // C: left half, stack 9 — overlaps A and sits on top.
    let node_a = ComputedNode { size: Vec2::new(400.0, 600.0), stack_index: 5, ..ComputedNode::DEFAULT };
    let tf_a = UiGlobalTransform::from_xy(200.0, 300.0);
    let node_b = ComputedNode { size: Vec2::new(400.0, 600.0), stack_index: 3, ..ComputedNode::DEFAULT };
    let tf_b = UiGlobalTransform::from_xy(600.0, 300.0);
    let node_c = ComputedNode { size: Vec2::new(400.0, 600.0), stack_index: 9, ..ComputedNode::DEFAULT };
    let tf_c = UiGlobalTransform::from_xy(200.0, 300.0);
    let candidates = [(a, &node_a, &tf_a), (b, &node_b, &tf_b), (c, &node_c, &tf_c)];

    assert_eq!(
        topmost_terminal_at(Vec2::new(600.0, 300.0), candidates.iter().copied()),
        Some(b),
        "a point only B contains must resolve to B"
    );
    assert_eq!(
        topmost_terminal_at(Vec2::new(100.0, 300.0), candidates.iter().copied()),
        Some(c),
        "where A and C overlap, the higher stack_index (C) wins"
    );
    assert_eq!(
        topmost_terminal_at(Vec2::new(2000.0, 2000.0), candidates.iter().copied()),
        None,
        "a point outside every node resolves to None"
    );
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p ozma_terminal --lib mouse::tests::topmost_terminal_at`
Expected: FAIL — `cannot find function topmost_terminal_at`.

- [ ] **Step 4: Implement `topmost_terminal_at`**

Add to `mouse.rs` (place it just below `cell_at_cursor`, ~line 184):

```rust
/// The `Entity` of the topmost `OzmaTerminal` whose node contains `cursor_phys`,
/// or `None` when the cursor is over none. "Topmost" is the highest
/// `ComputedNode::stack_index` (Bevy's resolved front-to-back UI order); a higher
/// index is drawn later, i.e. on top.
pub(crate) fn topmost_terminal_at<'a>(
    cursor_phys: Vec2,
    candidates: impl Iterator<Item = (Entity, &'a ComputedNode, &'a UiGlobalTransform)>,
) -> Option<Entity> {
    candidates
        .filter(|&(_, node, transform)| node.contains_point(*transform, cursor_phys))
        .max_by_key(|&(_, node, _)| node.stack_index())
        .map(|(entity, _, _)| entity)
}
```

- [ ] **Step 5: Rewrite `dispatch_mouse_buttons` for multiple terminals**

Replace the entire `dispatch_mouse_buttons` function (current lines 359-470) with:

```rust
/// The crate's mouse-button dispatcher. Hit-tests the topmost terminal under the
/// cursor on press, locks drag/release to that terminal, tracks clicks and drag
/// state, drives `decide_button`, and triggers `TerminalMouseEffects`. Skips any
/// `OzmaTerminal` carrying `MouseDisabled`; an empty candidate set (modal
/// suppression) drains events and resets the gesture.
pub(crate) fn dispatch_mouse_buttons(
    mut commands: Commands,
    mut gesture: ResMut<OzmaMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    terminals: Query<
        (
            Entity,
            &TerminalHandle,
            &ComputedNode,
            &UiGlobalTransform,
            &TerminalGrid,
        ),
        (With<OzmaTerminal>, Without<MouseDisabled>),
    >,
    cfg: Res<OzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        buttons.clear();
        gesture.drag = None;
        gesture.held = None;
        return;
    };
    if !window.focused || terminals.is_empty() {
        buttons.clear();
        gesture.drag = None;
        gesture.held = None;
        return;
    }
    let scale = window.scale_factor();
    let Some(cursor_phys) = window.cursor_position().map(|c| c * scale) else {
        buttons.clear();
        gesture.drag = None;
        gesture.held = None;
        return;
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let mods = protocol_mods(&keys);
    let modifier_held = link_modifier_held(&mods);

    for ev in buttons.read() {
        let kind = match ev.state {
            ButtonState::Pressed => ButtonEventKind::Press,
            ButtonState::Released => ButtonEventKind::Release,
        };
        let target = if kind == ButtonEventKind::Press {
            topmost_terminal_at(
                cursor_phys,
                terminals.iter().map(|(e, _, node, transform, _)| (e, node, transform)),
            )
        } else {
            gesture.held.map(|h| h.entity)
        };
        let Some(target) = target else {
            continue;
        };
        let Ok((_, handle, node, transform, grid)) = terminals.get(target) else {
            gesture.held = None;
            gesture.drag = None;
            continue;
        };
        let ctx = CellContext {
            node,
            transform,
            grid,
            cell_w,
            cell_h,
        };
        let modes = handle.current_modes();
        let Some((evt, link)) = resolve_button_event(
            &mut gesture,
            &ctx,
            ev,
            cursor_phys,
            scale,
            modifier_held,
            time.elapsed(),
            &cfg,
        ) else {
            continue;
        };
        let decided = decide_button(
            &mut gesture,
            modes,
            evt,
            mods,
            modifier_held,
            link,
            &cfg.buttons,
        );
        let opened = matches!(decided.as_slice(), [MouseEffect::OpenUri(_)]);
        match evt.kind {
            ButtonEventKind::Press if !opened => {
                gesture.held = Some(HeldPointer {
                    entity: target,
                    button: evt.button,
                    last_cell: evt.cell,
                });
            }
            ButtonEventKind::Release => gesture.held = None,
            _ => {}
        }
        if !decided.is_empty() {
            commands.trigger(TerminalMouseEffects {
                entity: target,
                effects: decided,
            });
        }
    }

    let Some(held) = gesture.held else {
        return;
    };
    let Ok((_, handle, node, transform, grid)) = terminals.get(held.entity) else {
        gesture.held = None;
        gesture.drag = None;
        return;
    };
    let ctx = CellContext {
        node,
        transform,
        grid,
        cell_w,
        cell_h,
    };
    let modes = handle.current_modes();
    if let Some((drag_effects, new_cell)) = synthesize_drag(
        &mut gesture,
        &ctx,
        cursor_phys,
        modes,
        mods,
        modifier_held,
        &cfg.buttons,
    ) {
        if let Some(h) = gesture.held.as_mut() {
            h.last_cell = new_cell;
        }
        if !drag_effects.is_empty() {
            commands.trigger(TerminalMouseEffects {
                entity: held.entity,
                effects: drag_effects,
            });
        }
    }
}
```

- [ ] **Step 6: Run the crate tests**

Run: `cargo test -p ozma_terminal`
Expected: PASS — `topmost_terminal_at` test passes; `mouse_disabled_terminal_drains_without_arming_a_gesture` still passes (empty query drains); all `decide_button` tests unchanged.

- [ ] **Step 7: Build + lint the workspace**

Run: `cargo build && cargo clippy -p ozma_terminal`
Expected: PASS, no warnings (the `entity` field is now read in the drag-synthesis lookup).

- [ ] **Step 8: Commit**

```bash
git add crates/ozma_terminal/src/mouse.rs
git commit -m "feat(ozma_terminal): topmost hit-test + entity-locked button dispatch

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Multi-terminal `dispatch_mouse_wheel` + accumulator retarget

Route the wheel to the topmost terminal under the cursor (once per frame — the wheel already coalesces deltas), and reset the sub-notch residual when the wheel target changes so a fraction cannot bleed between terminals.

**Files:**
- Modify: `crates/ozma_terminal/src/mouse.rs`

**Interfaces:**
- Consumes: `topmost_terminal_at` (Task 2), `accumulate_notches`, `wheel_delta_cells`, `decide_wheel`, `build_wheel_modifiers`.
- Produces: `WheelAccumulator { residual_cells: f32, last_target: Option<Entity> }` with `fn retarget(&mut self, entity: Entity)`.

- [ ] **Step 1: Add `last_target` to `WheelAccumulator` + a failing retarget test**

Replace the `WheelAccumulator` struct (current lines 303-306) with:

```rust
/// Carries the sub-notch wheel remainder across frames, scoped to the last
/// terminal the wheel targeted.
#[derive(Resource, Default)]
pub(crate) struct WheelAccumulator {
    residual_cells: f32,
    last_target: Option<Entity>,
}

impl WheelAccumulator {
    /// Resets the residual when the wheel target changes, so a sub-notch fraction
    /// accumulated over one terminal cannot bleed into the next.
    fn retarget(&mut self, entity: Entity) {
        if self.last_target != Some(entity) {
            self.residual_cells = 0.0;
            self.last_target = Some(entity);
        }
    }
}
```

Add to `mouse.rs` `mod tests`:

```rust
#[test]
fn wheel_accumulator_resets_residual_on_target_change() {
    let mut world = World::new();
    let a = world.spawn_empty().id();
    let b = world.spawn_empty().id();
    let mut acc = WheelAccumulator::default();
    acc.retarget(a);
    assert_eq!(accumulate_notches(&mut acc, 0.3, 0.5), 0);
    acc.retarget(a);
    assert_eq!(accumulate_notches(&mut acc, 0.3, 0.5), 1, "0.3 + 0.3 = 0.6 → one notch on the same target");
    acc.retarget(b);
    assert_eq!(accumulate_notches(&mut acc, 0.3, 0.5), 0, "switching target clears the carried residual");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozma_terminal --lib mouse::tests::wheel_accumulator_resets`
Expected: FAIL — `no method named retarget` / missing field.

- [ ] **Step 3: Rewrite `dispatch_mouse_wheel` for multiple terminals**

Replace the entire `dispatch_mouse_wheel` function (current lines 474-534) with:

```rust
/// The crate's wheel dispatcher: routes to the topmost terminal under the cursor,
/// resets the accumulator on a target change, accumulates notches, drives
/// `decide_wheel`, and triggers `TerminalMouseEffects`. Skips `MouseDisabled`
/// terminals; an empty candidate set drains the wheel events.
pub(crate) fn dispatch_mouse_wheel(
    mut commands: Commands,
    mut gesture_acc: ResMut<WheelAccumulator>,
    mut wheel: MessageReader<MouseWheel>,
    terminals: Query<
        (
            Entity,
            &TerminalHandle,
            &ComputedNode,
            &UiGlobalTransform,
            &TerminalGrid,
        ),
        (With<OzmaTerminal>, Without<MouseDisabled>),
    >,
    cfg: Res<OzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        wheel.clear();
        return;
    };
    if !window.focused || terminals.is_empty() {
        wheel.clear();
        return;
    }
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let Some(cursor_phys) = window.cursor_position().map(|c| c * window.scale_factor()) else {
        wheel.clear();
        return;
    };
    let Some(target) = topmost_terminal_at(
        cursor_phys,
        terminals.iter().map(|(e, _, node, transform, _)| (e, node, transform)),
    ) else {
        wheel.clear();
        return;
    };
    let Ok((_, handle, node, transform, grid)) = terminals.get(target) else {
        wheel.clear();
        return;
    };
    let ctx = CellContext {
        node,
        transform,
        grid,
        cell_w,
        cell_h,
    };

    gesture_acc.retarget(target);
    let delta_cells: f32 = wheel
        .read()
        .map(|ev| wheel_delta_cells(ev.unit, ev.y, ctx.cell_h))
        .sum();
    let raw = accumulate_notches(&mut gesture_acc, delta_cells, cfg.cells_per_notch);
    if raw == 0 {
        return;
    }
    // NOTE: Bevy +y (up/older) → engine convention (negative = up/older).
    let notches = -raw;
    let cell = ctx
        .hit(cursor_phys)
        .map(|(cell, _)| cell)
        .unwrap_or(CellCoord { col: 1, row: 1 });
    let mods = build_wheel_modifiers(&keys, &cfg);
    let effects = decide_wheel(handle.current_modes(), notches, cell, mods, &cfg.wheel);
    if !effects.is_empty() {
        commands.trigger(TerminalMouseEffects {
            entity: target,
            effects,
        });
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ozma_terminal`
Expected: PASS (retarget test + all existing wheel tests).

- [ ] **Step 5: Build + lint**

Run: `cargo build && cargo clippy -p ozma_terminal`
Expected: PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/ozma_terminal/src/mouse.rs
git commit -m "feat(ozma_terminal): wheel routes to topmost terminal; per-target residual

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Multi-terminal hyperlink hover

Make the hover-cursor system underline / set the icon on the topmost terminal under the cursor instead of the single terminal.

**Files:**
- Modify: `crates/ozma_terminal/src/hyperlink.rs`

**Interfaces:**
- Consumes: `topmost_terminal_at`, `cell_at_cursor` (both `crate::mouse`), `MouseDisabled`.
- Produces: no new public surface (internal rewrite of `resolve_hover`).

- [ ] **Step 1: Import `topmost_terminal_at`**

In `crates/ozma_terminal/src/hyperlink.rs`, extend the `crate::mouse` import (the line set in Task 1 Step 3) to:

```rust
use crate::mouse::{MouseDisabled, OzmaTerminalMouseSet, cell_at_cursor, protocol_mods, topmost_terminal_at};
```

- [ ] **Step 2: Rewrite `resolve_hover` to hit-test topmost**

Replace the entire `resolve_hover` function (current lines 95-133) with:

```rust
fn resolve_hover(
    hover: &mut HyperlinkHoverState,
    terminals: &Query<
        (Entity, &ComputedNode, &UiGlobalTransform, &TerminalGrid),
        (With<OzmaTerminal>, Without<MouseDisabled>),
    >,
    metrics: &TerminalCellMetricsResource,
    windows: &Query<&Window, With<PrimaryWindow>>,
    modifier_held: bool,
) -> SystemCursorIcon {
    let Ok(window) = windows.single() else {
        return SystemCursorIcon::Default;
    };
    let Some(cursor_phys) = window.cursor_position().map(|c| c * window.scale_factor()) else {
        return SystemCursorIcon::Default;
    };
    let Some(target) = topmost_terminal_at(
        cursor_phys,
        terminals.iter().map(|(e, node, transform, _)| (e, node, transform)),
    ) else {
        return SystemCursorIcon::Default;
    };
    let Ok((entity, node, transform, grid)) = terminals.get(target) else {
        return SystemCursorIcon::Default;
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let Some((cell, _side)) = cell_at_cursor(
        node,
        transform,
        cursor_phys,
        cell_w,
        cell_h,
        grid.cols,
        grid.rows,
    ) else {
        return SystemCursorIcon::Default;
    };
    let id = grid
        .hyperlink_at((cell.row - 1) as u16, (cell.col - 1) as u16)
        .map(|(id, _uri)| id);
    hover.entity = Some(entity);
    hover.hyperlink_id = id;
    cursor_decision(id.is_some(), modifier_held)
}
```

The caller `hyperlink_hover_cursor` passes its `terminal` query by reference as `&terminals`; its query type already matches the new parameter type after Task 1 Step 3 (it is `(With<OzmaTerminal>, Without<MouseDisabled>)`). Rename the parameter binding in `hyperlink_hover_cursor` from `terminal` to `terminals` for plural-query naming, and update the `resolve_hover(&mut hover, &terminals, …)` call accordingly.

- [ ] **Step 3: Build, test, lint**

Run: `cargo test -p ozma_terminal --lib hyperlink:: && cargo build && cargo clippy -p ozma_terminal`
Expected: PASS — the pure `link_modifier_matches_platform` / `cursor_decision_*` tests are unchanged; the crate builds.

- [ ] **Step 4: Commit**

```bash
git add crates/ozma_terminal/src/hyperlink.rs
git commit -m "feat(ozma_terminal): hover underlines the topmost terminal under the cursor

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Workspace verification + manual smoke

Confirm the whole workspace builds, lints, and tests, then verify multi-terminal mouse behavior in the running app (the dispatcher wiring — topmost routing, entity-locked drag, background scroll — cannot be unit-tested without a real PTY/window, so it is verified here).

**Files:** none (verification only).

- [ ] **Step 1: Full workspace gate**

Run: `cargo test && cargo clippy --workspace && cargo fmt --check`
Expected: PASS across all crates; `fmt --check` clean (run `cargo fmt` if not).

- [ ] **Step 2: Confirm the rename is complete**

Run: `grep -rn "InputDisabled\|maintain_input_disabled" src/ crates/`
Expected: no output.

- [ ] **Step 3: Manual smoke — single terminal (regression)**

Run `cargo run`. In the default Ozma terminal verify, unchanged from before: drag-select + copy, double/triple-click word/line select, wheel scrollback, Cmd-click an OSC-8 link, and that an `htop`/`vim`-style app receives mouse reports. Open the picker and confirm the terminal stops responding to the mouse (all-`MouseDisabled` suppression).

- [ ] **Step 4: Manual smoke — multiple terminals**

Temporarily spawn a second overlapping `OzmaTerminal` (e.g. a scratch `OnEnter(AppMode::Ozma)` system spawning a second `OzmaTerminalBundle` with a half-size `Node` + `GlobalZIndex(1)`), then verify: selection/scroll/links act on whichever terminal is under the cursor; a drag begun in the top terminal keeps extending that terminal's selection when the pointer moves over the other; and scrolling the lower (non-keyboard-focused) terminal works. Revert the scratch spawn before finishing.

- [ ] **Step 5: Final commit (if fmt or fixups were needed)**

```bash
git add -A
git commit -m "chore(ozma_terminal): fmt + multi-terminal mouse verification

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-review notes

- **Spec coverage:** §1 routing → Tasks 2/3/4 (`topmost_terminal_at`); §2 gesture lock → Task 2 (`HeldPointer.entity`); §3 gating split → Task 1; §3 wheel residual reset → Task 3 (`retarget`); §4 hover → Task 4; host change → Task 1; out-of-scope (keyboard `.single()`, layout, exit) untouched with the keyboard guardrail NOTE added in Task 1.
- **Type consistency:** `topmost_terminal_at` signature and `HeldPointer`/`WheelAccumulator` shapes are identical across Tasks 2–4. `decide_button` / `decide_wheel` / `CellContext` / `resolve_button_event` / `synthesize_drag` are reused unchanged.
- **Untestable-without-PTY note:** the dispatcher wiring is covered by the pure `topmost_terminal_at` + `decide_*` tests plus the `MouseDisabled` drain test and Task 5 manual smoke — matching the crate's existing testing approach (the `TerminalHandle` has no public constructor).
