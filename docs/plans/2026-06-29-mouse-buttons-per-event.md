# Per-operation Mouse EntityEvents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `MouseEffect` enum + `TerminalMouseEffects` carrier + single big-`match` apply observer with one `EntityEvent` per mouse operation and one focused observer each, while keeping the gather system single and the deciders pure.

**Architecture:** Additive-then-remove, in three green-at-each-step tasks. Task 1 adds the 7 new events + per-op observers + a shared `apply_to_terminal` helper to `ozma_terminal` ALONGSIDE the existing path (both registered; new path receives no triggers yet) and adds new apply-side tests. Task 2 switches the host gather side: `MouseEffect` becomes a host-private decision IR in `src/input/mouse.rs`, the deciders keep returning `Vec<MouseEffect>`, and a thin `trigger_mouse_effects` translation fans the Vec out to per-op `commands.trigger(...)` in order; host tests are reworked to observe the new events. Task 3 deletes the now-dead `MouseEffect`/`TerminalMouseEffects`/`on_terminal_mouse_effects`/`apply_effect`/`apply_effect_detached` and their exports from `ozma_terminal`.

**Tech Stack:** Rust 2024, Bevy 0.18 ECS (`EntityEvent` + observer idiom), `ozma_tty_engine` `TerminalHandle`, `cargo test`.

## Global Constraints

- Edition 2024, toolchain 1.95. Comments English-only.
- Rust rules (`.claude/rules/rust.md`): no `mod.rs`; comment taxonomy `// TODO:` / `// NOTE:` / `// SAFETY:` only; doc comments (`///`) on every externally-`pub` item and `//!` on each module file; all `use` at top in one contiguous block, no inline fully-qualified paths; visibility minimized (private unless a cross-module caller forces wider); item ordering `pub` before private; mutable params before immutable; `Plugin::build` is one method chain; register systems/observers in the defining file's plugin; no `_q` suffix on `Query` params.
- Bevy 0.18 ordering fact this design relies on: `commands.trigger(..)` enqueues a FIFO command and each trigger fully resolves (observers + their queued commands) before the next, so sequential triggers from one system body are observed in source order. One event = one observer here, so same-event observer-order arbitrariness does not apply.
- Each new event is `pub` (the host crate constructs and triggers them) with `pub` fields (no `new` constructor needed). `TerminalForwardInput` is preserved (consumed by `src/input/tmux/forward.rs`).
- After every task: `cargo test -p ozma_terminal` and/or `cargo test` (workspace) green, plus `cargo clippy --workspace --all-targets` clean and `cargo fmt`.

---

### Task 1: Add per-op events, observers, and shared helper to `ozma_terminal` (additive)

**Files:**
- Modify: `crates/ozma_terminal/src/mouse.rs` (add 7 events, `apply_to_terminal` helper, 7 observers, register them; KEEP the existing `MouseEffect`/`TerminalMouseEffects`/`on_terminal_mouse_effects`/`apply_effect`/`apply_effect_detached` for now)
- Modify: `crates/ozma_terminal/src/lib.rs:18` (export the 7 new events alongside the existing ones)
- Test: `crates/ozma_terminal/src/mouse.rs` `#[cfg(test)] mod tests` (add new tests for the new events)

**Interfaces:**
- Produces (consumed by Task 2's host translation, all `pub` with `pub` fields, all `#[derive(EntityEvent, Debug, Clone)]`):
  - `TerminalMouseWrite { entity: Entity, bytes: Vec<u8> }`
  - `TerminalSelectionStart { entity: Entity, point: Point, side: Side, ty: SelectionType }`
  - `TerminalSelectionUpdate { entity: Entity, point: Point, side: Side }`
  - `TerminalSelectionClear { entity: Entity }`
  - `TerminalSelectionCopy { entity: Entity }`
  - `TerminalViewportScroll { entity: Entity, lines: i32 }`
  - `TerminalOpenUri { entity: Entity, uri: String }`
  - (the `entity` field carries `#[event_target]` on each)
- Consumes: existing `TerminalHandle`, `PtyHandle`, `Coalescer`, `Point`, `Side`, `SelectionType` (already imported at `mouse.rs:13`), `Clipboard` (`crate::clipboard::Clipboard`), `try_open_uri` (`crate::hyperlink::try_open_uri`, `pub(crate)`), `OzmaTerminal`, `TerminalForwardInput`.

- [ ] **Step 1: Add the `Mut` import**

In `crates/ozma_terminal/src/mouse.rs`, the imports already include `use bevy::prelude::*;` (which re-exports `Mut`). No new `use` is required — verify `Mut` resolves; if not, the bevy prelude provides it. (No edit unless the build complains.)

- [ ] **Step 2: Write the failing tests for the new events**

Append these tests INSIDE the existing `#[cfg(test)] mod tests { ... }` block in `crates/ozma_terminal/src/mouse.rs` (it already has `use super::*;` and imports `Column`, `Line`):

```rust
    #[test]
    fn detached_write_event_forwards_bytes() {
        use ozma_tty_engine::TerminalHandle;

        #[derive(Resource, Default)]
        struct CapturedForward(Vec<Vec<u8>>);

        let mut app = App::new();
        app.init_resource::<Clipboard>()
            .init_resource::<CapturedForward>()
            .add_observer(on_terminal_mouse_write)
            .add_observer(
                |ev: On<TerminalForwardInput>, mut cap: ResMut<CapturedForward>| {
                    cap.0.push(ev.bytes.clone());
                },
            );

        let handle = TerminalHandle::detached(10, 5);
        let entity = app.world_mut().spawn((OzmaTerminal, handle)).id();

        app.world_mut().trigger(TerminalMouseWrite {
            entity,
            bytes: b"\x1b[<0;1;1M".to_vec(),
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<CapturedForward>().0,
            vec![b"\x1b[<0;1;1M".to_vec()],
            "TerminalMouseWrite on a PTY-less OzmaTerminal must emit TerminalForwardInput"
        );
    }

    #[test]
    fn detached_selection_start_event_sets_selection_via_vt_only() {
        use ozma_tty_engine::TerminalHandle;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Clipboard>()
            .add_observer(on_terminal_selection_start);

        let handle = TerminalHandle::detached(10, 5);
        let entity = app.world_mut().spawn((OzmaTerminal, handle)).id();

        app.world_mut().trigger(TerminalSelectionStart {
            entity,
            point: Point::new(Line(0), Column(0)),
            side: Side::Left,
            ty: SelectionType::Simple,
        });
        app.update();

        let handle = app.world().entity(entity).get::<TerminalHandle>().unwrap();
        assert!(
            handle.selection_to_string().is_some(),
            "TerminalSelectionStart on a PTY-less OzmaTerminal must set a selection via vt_only"
        );
    }

    #[test]
    fn viewport_scroll_event_on_missing_terminal_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Clipboard>()
            .add_observer(on_terminal_viewport_scroll);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .trigger(TerminalViewportScroll { entity, lines: 3 });
        app.update();
    }
```

- [ ] **Step 3: Run the tests to verify they fail (symbols not defined yet)**

Run: `cargo test -p ozma_terminal detached_write_event_forwards_bytes detached_selection_start_event_sets_selection_via_vt_only viewport_scroll_event_on_missing_terminal_does_not_panic 2>&1 | tail -20`
Expected: FAIL — compile errors `cannot find ... TerminalMouseWrite / on_terminal_mouse_write / ...`.

- [ ] **Step 4: Add the 7 event structs**

In `crates/ozma_terminal/src/mouse.rs`, immediately AFTER the existing `TerminalForwardInput` struct (ends at line 51, before `TerminalMouseEffects`), insert:

```rust
/// Writes mouse-protocol report bytes to `entity`'s backend (PTY when
/// attached, `TerminalForwardInput` when detached).
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalMouseWrite {
    /// The terminal entity whose backend receives `bytes`.
    #[event_target]
    pub entity: Entity,
    /// The report bytes to deliver.
    pub bytes: Vec<u8>,
}

/// Starts a new local selection on `entity` at `point`.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalSelectionStart {
    /// The terminal entity to start the selection on.
    #[event_target]
    pub entity: Entity,
    /// The viewport-relative anchor of the new selection.
    pub point: Point,
    /// Which half of the cell the anchor sits in.
    pub side: Side,
    /// The selection granularity (simple / semantic / lines).
    pub ty: SelectionType,
}

/// Extends `entity`'s current selection's moving end to `point`.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalSelectionUpdate {
    /// The terminal entity whose selection is extended.
    #[event_target]
    pub entity: Entity,
    /// The viewport-relative moving end.
    pub point: Point,
    /// Which half of the cell the moving end sits in.
    pub side: Side,
}

/// Clears any active local selection on `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalSelectionClear {
    /// The terminal entity whose selection is cleared.
    #[event_target]
    pub entity: Entity,
}

/// Copies `entity`'s current selection to the clipboard.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalSelectionCopy {
    /// The terminal entity whose selection is copied.
    #[event_target]
    pub entity: Entity,
}

/// Scrolls `entity`'s viewport by `lines` (negative = up / into history).
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalViewportScroll {
    /// The terminal entity to scroll.
    #[event_target]
    pub entity: Entity,
    /// Lines to scroll; negative scrolls up into scrollback.
    pub lines: i32,
}

/// Opens `uri` in the host browser / handler. `entity` is carried for
/// family uniformity; the apply does not read it.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalOpenUri {
    /// The terminal entity the link belongs to (unused by the apply).
    #[event_target]
    pub entity: Entity,
    /// The URI to open.
    pub uri: String,
}
```

- [ ] **Step 5: Add the shared `apply_to_terminal` helper and the 7 observers**

In `crates/ozma_terminal/src/mouse.rs`, insert the following AFTER `on_terminal_mouse_effects` and its `apply_effect` / `apply_effect_detached` (i.e. after line 204, before the `#[cfg(test)]` module). These are added alongside the existing code:

```rust
/// Applies one handle-touching mouse op to `entity`, branching on whether
/// the terminal is PTY-attached (apply through the coalescer) or detached
/// (mutate the VT only, then `flush_emit`). `detached` returns whether a
/// frame flush is needed (the write op forwards instead and returns false).
fn apply_to_terminal(
    commands: &mut Commands,
    handle: &mut TerminalHandle,
    pty: Option<Mut<PtyHandle>>,
    coalescer: Option<Mut<Coalescer>>,
    entity: Entity,
    attached: impl FnOnce(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
    detached: impl FnOnce(&mut Commands, &mut TerminalHandle, Entity) -> bool,
) {
    if let (Some(mut pty), Some(mut coalescer)) = (pty, coalescer) {
        attached(handle, &mut pty, &mut coalescer);
    } else if detached(commands, handle, entity) {
        handle.flush_emit(commands, entity);
    }
}

/// Applies a `TerminalMouseWrite`: PTY write when attached, otherwise a
/// `TerminalForwardInput` to the host-owned backend router.
fn on_terminal_mouse_write(
    ev: On<TerminalMouseWrite>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, pty, _coalescer| {
            if let Err(e) = handle.write(pty, &ev.bytes) {
                tracing::warn!(?e, "ozma mouse pty write failed");
            }
        },
        |commands, _handle, entity| {
            commands.trigger(TerminalForwardInput {
                entity,
                bytes: ev.bytes.clone(),
            });
            false
        },
    );
}

/// Applies a `TerminalSelectionStart`.
fn on_terminal_selection_start(
    ev: On<TerminalSelectionStart>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, _pty, coalescer| handle.selection_start_at(coalescer, ev.point, ev.side, ev.ty),
        |_commands, handle, _entity| {
            handle.selection_start_at_vt_only(ev.point, ev.side, ev.ty);
            true
        },
    );
}

/// Applies a `TerminalSelectionUpdate`.
fn on_terminal_selection_update(
    ev: On<TerminalSelectionUpdate>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, _pty, coalescer| handle.selection_update_to(coalescer, ev.point, ev.side),
        |_commands, handle, _entity| {
            handle.selection_update_to_vt_only(ev.point, ev.side);
            true
        },
    );
}

/// Applies a `TerminalSelectionClear`.
fn on_terminal_selection_clear(
    ev: On<TerminalSelectionClear>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, _pty, coalescer| handle.selection_clear(coalescer),
        |_commands, handle, _entity| {
            handle.selection_clear_vt_only();
            true
        },
    );
}

/// Applies a `TerminalViewportScroll`.
fn on_terminal_viewport_scroll(
    ev: On<TerminalViewportScroll>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, _pty, coalescer| handle.scroll(coalescer, ev.lines),
        |_commands, handle, _entity| {
            handle.scroll_vt_only(ev.lines);
            true
        },
    );
}

/// Applies a `TerminalSelectionCopy`: writes the selection text (if any) to
/// the clipboard. Needs only read access to the handle.
fn on_terminal_selection_copy(
    ev: On<TerminalSelectionCopy>,
    mut clipboard: ResMut<Clipboard>,
    terminals: Query<&TerminalHandle, With<OzmaTerminal>>,
) {
    let Ok(handle) = terminals.get(ev.entity) else {
        return;
    };
    if let Some(text) = handle.selection_to_string() {
        clipboard.write(text);
    }
}

/// Applies a `TerminalOpenUri`: opens the link in the host handler.
fn on_terminal_open_uri(ev: On<TerminalOpenUri>) {
    try_open_uri(&ev.uri);
}
```

- [ ] **Step 6: Register the 7 observers in `OzmaMousePlugin`**

In `crates/ozma_terminal/src/mouse.rs`, replace the existing `impl Plugin for OzmaMousePlugin` body (currently lines 80-84):

```rust
impl Plugin for OzmaMousePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_mouse_effects);
    }
}
```

with (keeps the old observer, adds the 7 new ones — one method chain):

```rust
impl Plugin for OzmaMousePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_mouse_effects)
            .add_observer(on_terminal_mouse_write)
            .add_observer(on_terminal_selection_start)
            .add_observer(on_terminal_selection_update)
            .add_observer(on_terminal_selection_clear)
            .add_observer(on_terminal_selection_copy)
            .add_observer(on_terminal_viewport_scroll)
            .add_observer(on_terminal_open_uri);
    }
}
```

- [ ] **Step 7: Export the new events from `lib.rs`**

In `crates/ozma_terminal/src/lib.rs`, replace line 18:

```rust
pub use mouse::{MouseEffect, TerminalForwardInput, TerminalMouseEffects};
```

with:

```rust
pub use mouse::{
    MouseEffect, TerminalForwardInput, TerminalMouseEffects, TerminalMouseWrite, TerminalOpenUri,
    TerminalSelectionClear, TerminalSelectionCopy, TerminalSelectionStart, TerminalSelectionUpdate,
    TerminalViewportScroll,
};
```

- [ ] **Step 8: Run the new tests to verify they pass**

Run: `cargo test -p ozma_terminal 2>&1 | tail -25`
Expected: all green, including the 3 new tests and all pre-existing tests.

- [ ] **Step 9: Clippy + fmt**

Run: `cargo clippy -p ozma_terminal --all-targets 2>&1 | tail -15 && cargo fmt`
Expected: no warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/ozma_terminal/src/mouse.rs crates/ozma_terminal/src/lib.rs
git commit -m "feat(ozma_terminal): add per-operation mouse EntityEvents + observers (additive)"
```

---

### Task 2: Switch the host gather side to per-op event triggers

**Files:**
- Modify: `src/input/mouse.rs` (define host-private `MouseEffect`; change the import at line 26; add `trigger_mouse_effects`; replace the 3 trigger sites; rework the integration tests)

**Interfaces:**
- Consumes (from Task 1, imported from `ozma_terminal`): `TerminalMouseWrite`, `TerminalSelectionStart`, `TerminalSelectionUpdate`, `TerminalSelectionClear`, `TerminalSelectionCopy`, `TerminalViewportScroll`, `TerminalOpenUri`, `OzmaTerminal`, `TerminalForwardInput`.
- Produces: a host-private `enum MouseEffect` (same variants as before) used only as the deciders' return IR; `fn trigger_mouse_effects(commands: &mut Commands, entity: Entity, effects: Vec<MouseEffect>)`.

- [ ] **Step 1: Move `MouseEffect` into the host as a private enum and fix imports**

In `src/input/mouse.rs`, replace the import at line 26:

```rust
use ozma_terminal::{MouseEffect, OzmaTerminal, TerminalMouseEffects};
```

with (drop `MouseEffect` + `TerminalMouseEffects`, add the 7 events):

```rust
use ozma_terminal::{
    OzmaTerminal, TerminalForwardInput, TerminalMouseWrite, TerminalOpenUri,
    TerminalSelectionClear, TerminalSelectionCopy, TerminalSelectionStart, TerminalSelectionUpdate,
    TerminalViewportScroll,
};
```

(`TerminalForwardInput` is only needed by tests; if clippy flags it unused in non-test builds, scope it to the test module instead — see Step 6.)

Then, define the host-private enum. Add it directly ABOVE the `MouseInputPlugin` struct (around line 36, after the `use std::time::Duration;` import block). Use the same variants the deciders already build:

```rust
/// Host-private decision IR: the deciders (`decide_button` / `decide_wheel`)
/// return an ordered `Vec` of these, which `trigger_mouse_effects` fans out
/// to per-operation `EntityEvent`s on the target terminal.
#[derive(Debug, Clone, PartialEq)]
enum MouseEffect {
    Write(Vec<u8>),
    SelStart {
        point: Point,
        side: Side,
        ty: SelectionType,
    },
    SelUpdate {
        point: Point,
        side: Side,
    },
    SelClear,
    Copy,
    Scroll(i32),
    OpenUri(String),
}
```

`Point`, `Side`, `SelectionType` are already in scope: `Point`, `Side` via `ozma_tty_engine::{... Point, ... Side, ...}` at line 27-31; `SelectionType` is currently used only in tests via `ozma_tty_engine::SelectionType`. Add `SelectionType` to the `ozma_tty_engine` import group at the top (line 27-31) so the enum and the deciders resolve it:

Change the `ozma_tty_engine` import to include `SelectionType` (insert it alphabetically), e.g.:

```rust
use ozma_tty_engine::{
    ButtonAction, ButtonConfig, ButtonEvent, ButtonEventKind, CellCoord, Column, Line,
    MouseButtonKind, Point, ProtocolModifiers, SelectionType, Side, TermMode, TerminalHandle,
    TerminalModifiers, WheelAction, WheelConfig, WheelModifiers,
};
```

- [ ] **Step 2: Add the `trigger_mouse_effects` translation**

In `src/input/mouse.rs`, add this private fn. Place it just below `effects_from_wheel_action` (around line 428) so it sits with the other decision/translation helpers:

```rust
/// Fans an ordered `Vec<MouseEffect>` out to per-operation `EntityEvent`s on
/// `entity`, preserving order (Bevy's command queue is FIFO and each trigger
/// resolves before the next).
fn trigger_mouse_effects(commands: &mut Commands, entity: Entity, effects: Vec<MouseEffect>) {
    for effect in effects {
        match effect {
            MouseEffect::Write(bytes) => commands.trigger(TerminalMouseWrite { entity, bytes }),
            MouseEffect::SelStart { point, side, ty } => {
                commands.trigger(TerminalSelectionStart {
                    entity,
                    point,
                    side,
                    ty,
                });
            }
            MouseEffect::SelUpdate { point, side } => {
                commands.trigger(TerminalSelectionUpdate {
                    entity,
                    point,
                    side,
                });
            }
            MouseEffect::SelClear => commands.trigger(TerminalSelectionClear { entity }),
            MouseEffect::Copy => commands.trigger(TerminalSelectionCopy { entity }),
            MouseEffect::Scroll(lines) => commands.trigger(TerminalViewportScroll { entity, lines }),
            MouseEffect::OpenUri(uri) => commands.trigger(TerminalOpenUri { entity, uri }),
        }
    }
}
```

- [ ] **Step 3: Replace the 3 `TerminalMouseEffects` trigger sites**

In `src/input/mouse.rs`:

(a) In `dispatch_mouse_buttons`, replace (currently lines 197-199):

```rust
        if !decided.is_empty() {
            commands.trigger(TerminalMouseEffects::new(target, decided));
        }
```

with:

```rust
        trigger_mouse_effects(&mut commands, target, decided);
```

(b) In `dispatch_mouse_buttons`'s drag tail, replace (currently lines 231-233):

```rust
        if !drag_effects.is_empty() {
            commands.trigger(TerminalMouseEffects::new(held.entity, drag_effects));
        }
```

with:

```rust
        trigger_mouse_effects(&mut commands, held.entity, drag_effects);
```

(c) In `dispatch_mouse_wheel`, replace (currently lines 340-342):

```rust
    if !effects.is_empty() {
        commands.trigger(TerminalMouseEffects::new(target, effects));
    }
```

with:

```rust
    trigger_mouse_effects(&mut commands, target, effects);
```

- [ ] **Step 4: Run the full host build to surface test breakage**

Run: `cargo test --bin ozmux mouse 2>&1 | tail -30`
Expected: production code compiles; the integration tests in `mod tests` FAIL to compile because they still reference `TerminalMouseEffects` and `CapturedEffects(Vec<Vec<MouseEffect>>)`. The decider unit tests (`decide_button` / `decide_wheel` / `effects_from_wheel_action`) still compile (they use the now-host-private `MouseEffect`). Proceed to fix the integration tests.

- [ ] **Step 5: Rework `CapturedEffects` and the two test-app builders**

In `src/input/mouse.rs` `#[cfg(test)] mod tests`:

(a) Replace the `CapturedEffects` resource (currently lines 717-718):

```rust
    #[derive(Resource, Default)]
    struct CapturedEffects(Vec<Vec<MouseEffect>>);
```

with a flat capture that re-materializes each observed per-op event back into the host `MouseEffect` IR so the existing assertions keep working:

```rust
    #[derive(Resource, Default)]
    struct CapturedEffects(Vec<MouseEffect>);

    fn add_effect_capture_observers(app: &mut App) {
        app.add_observer(|ev: On<TerminalMouseWrite>, mut cap: ResMut<CapturedEffects>| {
            cap.0.push(MouseEffect::Write(ev.bytes.clone()));
        })
        .add_observer(
            |ev: On<TerminalSelectionStart>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::SelStart {
                    point: ev.point,
                    side: ev.side,
                    ty: ev.ty,
                });
            },
        )
        .add_observer(
            |ev: On<TerminalSelectionUpdate>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::SelUpdate {
                    point: ev.point,
                    side: ev.side,
                });
            },
        )
        .add_observer(
            |_ev: On<TerminalSelectionClear>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::SelClear);
            },
        )
        .add_observer(
            |_ev: On<TerminalSelectionCopy>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::Copy);
            },
        )
        .add_observer(
            |ev: On<TerminalViewportScroll>, mut cap: ResMut<CapturedEffects>| {
                cap.0.push(MouseEffect::Scroll(ev.lines));
            },
        )
        .add_observer(|ev: On<TerminalOpenUri>, mut cap: ResMut<CapturedEffects>| {
            cap.0.push(MouseEffect::OpenUri(ev.uri.clone()));
        });
    }
```

(b) In `make_selection_app` (currently lines 720-765), replace the single observer registration:

```rust
            .add_observer(
                |ev: On<TerminalMouseEffects>, mut cap: ResMut<CapturedEffects>| {
                    cap.0.push(ev.effects().to_vec());
                },
            )
            .add_systems(Update, dispatch_mouse_buttons);
```

with:

```rust
            .add_systems(Update, dispatch_mouse_buttons);
        add_effect_capture_observers(&mut app);
```

(c) In `make_wheel_app` (currently lines 1426-1470), do the same replacement of its observer:

```rust
            .add_observer(
                |ev: On<TerminalMouseEffects>, mut cap: ResMut<CapturedEffects>| {
                    cap.0.push(ev.effects().to_vec());
                },
            )
            .add_systems(Update, dispatch_mouse_wheel);
```

with:

```rust
            .add_systems(Update, dispatch_mouse_wheel);
        add_effect_capture_observers(&mut app);
```

(Note: both builders currently `return app;` at the end — keep that. Insert the `add_effect_capture_observers(&mut app);` call before the `app` is returned, i.e. after the spawn/window setup, replacing the chained observer as shown.)

- [ ] **Step 6: Fix the capture-reading assertions (flat Vec, not Vec<Vec>)**

The existing assertions use `cap.0.iter().flatten()`. With a flat `Vec<MouseEffect>`, drop `.flatten()`. Apply these edits in `src/input/mouse.rs` tests:

(a) `drag_survives_cursor_leaving_window` — change:
```rust
        let pinned = cap
            .0
            .iter()
            .flatten()
            .any(|e| matches!(e, MouseEffect::SelUpdate { point, .. } if point.column.0 == 99));
```
to:
```rust
        let pinned = cap
            .0
            .iter()
            .any(|e| matches!(e, MouseEffect::SelUpdate { point, .. } if point.column.0 == 99));
```

(b) `release_after_leaving_window_copies` — change:
```rust
            cap.0
                .iter()
                .flatten()
                .any(|e| matches!(e, MouseEffect::Copy)),
```
to:
```rust
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Copy)),
```

(c) `dispatch_pure_horizontal_right_emits_sgr_67` — change:
```rust
            cap.0
                .iter()
                .flatten()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<67;"))),
```
to:
```rust
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<67;"))),
```

(d) `dispatch_horizontal_left_emits_sgr_66` — change:
```rust
            cap.0
                .iter()
                .flatten()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<66;"))),
```
to:
```rust
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<66;"))),
```

(e) `dispatch_horizontal_without_mouse_mode_emits_no_report` — change:
```rust
            cap.0
                .iter()
                .flatten()
                .all(|e| !matches!(e, MouseEffect::Write(_))),
```
to:
```rust
            cap.0
                .iter()
                .all(|e| !matches!(e, MouseEffect::Write(_))),
```

(f) `pixel_horizontal_sensitivity_matches_vertical` — change the closure that flattens:
```rust
            cap.0
                .iter()
                .flatten()
                .filter_map(|e| match e {
                    MouseEffect::Write(b) => Some(b),
                    _ => None,
                })
```
to:
```rust
            cap.0
                .iter()
                .filter_map(|e| match e {
                    MouseEffect::Write(b) => Some(b),
                    _ => None,
                })
```

- [ ] **Step 7: Rewrite the "one trigger" diagonal test to per-event semantics**

`dispatch_diagonal_emits_both_axes_in_one_trigger` asserts `cap.0.len() == 1` (one carrier event holding both axes). Per-op events make vertical and horizontal two separate `TerminalMouseWrite` events, so the "one trigger" invariant no longer holds. Replace the whole test (currently lines 1517-1543) with:

```rust
    #[test]
    fn dispatch_diagonal_emits_both_axes() {
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5, -0.5);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<65;"))),
            "vertical (down, cb 65) report missing: {:?}",
            cap.0
        );
        assert!(
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<67;"))),
            "horizontal (right, cb 67) report missing: {:?}",
            cap.0
        );
    }
```

- [ ] **Step 8: Run the host tests to verify they pass**

Run: `cargo test --bin ozmux mouse 2>&1 | tail -30`
Expected: all mouse tests green (deciders, gesture integration, wheel).

- [ ] **Step 9: Clippy + fmt**

Run: `cargo clippy --bin ozmux --all-targets 2>&1 | tail -20 && cargo fmt`
Expected: no warnings. If `TerminalForwardInput` is reported unused in non-test code, move it from the top-level `use` into the `#[cfg(test)] mod tests` import block (it is only referenced by tests in the host).

- [ ] **Step 10: Commit**

```bash
git add src/input/mouse.rs
git commit -m "refactor(input): trigger per-op mouse events; MouseEffect now host-private IR"
```

---

### Task 3: Remove the dead legacy apply path from `ozma_terminal`

**Files:**
- Modify: `crates/ozma_terminal/src/mouse.rs` (delete `MouseEffect`, `TerminalMouseEffects`, `impl TerminalMouseEffects`, `on_terminal_mouse_effects`, `apply_effect`, `apply_effect_detached`, and the two legacy tests; remove the legacy observer registration; update the module `//!` doc)
- Modify: `crates/ozma_terminal/src/lib.rs:18` (drop `MouseEffect` and `TerminalMouseEffects` from the export)

**Interfaces:**
- Consumes: nothing new.
- Produces: a smaller public surface — `MouseEffect` and `TerminalMouseEffects` no longer exist.

- [ ] **Step 1: Confirm nothing else references the legacy types**

Run: `grep -rn "MouseEffect\b\|TerminalMouseEffects" --include=*.rs src/ crates/ | grep -v "src/input/mouse.rs"`
Expected: only matches inside `crates/ozma_terminal/src/mouse.rs` (the code being deleted). `src/input/mouse.rs` now has its OWN private `MouseEffect` — that is fine and is excluded. (`TmuxMouseEffect`/`TmuxMouseEffects` are a different family and must NOT match `\bMouseEffect\b`/`TerminalMouseEffects`; verify none appear.)

- [ ] **Step 2: Delete the legacy `MouseEffect` enum**

In `crates/ozma_terminal/src/mouse.rs`, delete the entire `pub enum MouseEffect { ... }` (currently lines 15-38, including its `///` docs above it starting at line 15).

- [ ] **Step 3: Delete `TerminalMouseEffects`, its `impl`, and the legacy observer + apply fns**

In `crates/ozma_terminal/src/mouse.rs`, delete:
- the `pub struct TerminalMouseEffects { ... }` and its doc (currently lines 53-63),
- the `impl TerminalMouseEffects { ... }` block (currently lines 65-75),
- the `on_terminal_mouse_effects` fn (currently lines 86-130),
- the `apply_effect` fn (currently lines 132-160),
- the `apply_effect_detached` fn (currently lines 162-204).

Keep: `TerminalForwardInput`, all 7 new events, `apply_to_terminal`, and the 7 new observers.

- [ ] **Step 4: Remove the legacy observer from `OzmaMousePlugin`**

In `crates/ozma_terminal/src/mouse.rs`, change the plugin chain so it no longer registers `on_terminal_mouse_effects`. The chain must start at the first remaining observer:

```rust
impl Plugin for OzmaMousePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_mouse_write)
            .add_observer(on_terminal_selection_start)
            .add_observer(on_terminal_selection_update)
            .add_observer(on_terminal_selection_clear)
            .add_observer(on_terminal_selection_copy)
            .add_observer(on_terminal_viewport_scroll)
            .add_observer(on_terminal_open_uri);
    }
}
```

- [ ] **Step 5: Delete the two legacy apply tests**

In `crates/ozma_terminal/src/mouse.rs` `#[cfg(test)] mod tests`, delete:
- `detached_terminal_forwards_write_and_selects_via_vt_only` (currently lines 211-255),
- `mouse_effects_on_entity_without_terminal_does_not_panic` (currently lines 257-271).

Keep the 3 tests added in Task 1.

- [ ] **Step 6: Update the module doc and the export**

(a) In `crates/ozma_terminal/src/mouse.rs`, update the `//!` header (currently lines 1-7) to describe the new shape:

```rust
//! Mouse-effect apply path for the Ozma terminal: the per-operation
//! `EntityEvent`s (`TerminalMouseWrite`, `TerminalSelection{Start,Update,
//! Clear,Copy}`, `TerminalViewportScroll`, `TerminalOpenUri`) plus the
//! `TerminalForwardInput` backend-bytes event, and one focused apply
//! observer per event that writes to the `TerminalHandle` / `Clipboard`
//! (or forwards to a PTY-less backend). The mode-neutral mouse dispatch
//! that DECIDES and triggers these lives in the host (`crate::input::mouse`
//! in the binary), scheduled in `InputPhase::Dispatch`.
```

(b) In `crates/ozma_terminal/src/lib.rs`, change line 18 to drop the two removed names:

```rust
pub use mouse::{
    TerminalForwardInput, TerminalMouseWrite, TerminalOpenUri, TerminalSelectionClear,
    TerminalSelectionCopy, TerminalSelectionStart, TerminalSelectionUpdate, TerminalViewportScroll,
};
```

- [ ] **Step 7: Run the full workspace test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: all green. (Per the `ozmux-test-gotchas` memory, a pre-existing IME test may fail and parallel teardown may SIGSEGV unrelated to this change; if so, re-run the mouse-relevant crates: `cargo test -p ozma_terminal && cargo test --bin ozmux mouse`.)

- [ ] **Step 8: Clippy + fmt across the workspace**

Run: `cargo clippy --workspace --all-targets 2>&1 | tail -25 && cargo fmt`
Expected: no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/ozma_terminal/src/mouse.rs crates/ozma_terminal/src/lib.rs
git commit -m "refactor(ozma_terminal): remove legacy MouseEffect/TerminalMouseEffects apply path"
```

---

## Notes for the implementer

- The host gather system (`dispatch_mouse_buttons`) is intentionally NOT split — it owns a single sequential `OzmaMouseGesture` state machine over one `MessageReader<MouseButtonInput>`. Do not attempt to split it; only the apply side is per-event.
- `decide_button` / `decide_wheel` / `effects_from_wheel_action` and ALL their unit tests are unchanged — they keep returning `Vec<MouseEffect>` against the host-private enum.
- Ordering: the per-op triggers are emitted in `Vec` order inside `trigger_mouse_effects`; Bevy 0.18's FIFO command queue guarantees they are observed in that order (e.g. `SelClear` before `Write`). Each event has exactly one observer, so same-event observer-order arbitrariness is irrelevant.
- `try_open_uri` stays `pub(crate)` in `ozma_terminal`; `TerminalOpenUri`'s observer lives in `ozma_terminal` for that reason.
