# Horizontal mouse-wheel / trackpad scroll — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Translate horizontal wheel/trackpad input (`ev.x`) into SGR/X10 horizontal wheel reports (`cb=66` left / `cb=67` right) for mouse-mode applications (e.g. Neovim with `set mouse=a` + `nowrap`), in both Default and tmux modes.

**Architecture:** Add a dedicated horizontal route function to the pure engine wheel router (`WheelAction::route_horizontal`), leaving the vertical `route` untouched. In the host wheel dispatcher, accumulate the horizontal axis on its own residual and emit reports through the existing `MouseEffect::Write` → `TerminalMouseEffects` → tmux `send-keys -H` / PTY path. No new effect/event types, no new config keys.

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18 ECS, `alacritty_terminal::TermMode`. Crates: `orzma_tty_engine` (pure wheel routing + SGR/X10 encoder), `orzma_terminal` (Bevy wheel dispatcher).

## Global Constraints

- Edition 2024, toolchain pinned to 1.95. Workspace builds with `cargo build`.
- **Comments:** only `// TODO:` / `// NOTE:` / `// SAFETY:`; `// NOTE:` is for critical caveats only (concrete harm if overlooked). All comments in English. No block comments, no commented-out code, no narrative comments.
- **Doc comments:** every externally-`pub` item gets a `///` one-line summary. (`route_horizontal` is `pub`; the private helpers are not required to have docs but a one-line `///` is welcome.)
- **Visibility:** narrowest that compiles. `route_horizontal` is `pub` (called cross-crate from `orzma_terminal`). `emit_protocol_reports`, `effects_from_wheel_action`, `build_wheel_modifiers_horizontal` are private (used only in their defining file). New struct fields stay private.
- **Parameter ordering:** mutable params before immutable (`accumulate_notches(residual: &mut f32, …)` keeps the `&mut` first).
- **Item ordering:** `pub` before private within an `impl`/module.
- No `mod.rs`. Imports at top of file, single contiguous block, no inline fully-qualified paths.
- `cargo clippy --workspace` and `cargo fmt` must be clean (run `just fix-lint` or the per-crate equivalents).
- **Feature scope:** mouse-mode applications only (`MOUSE_REPORT_CLICK | MOUSE_DRAG | MOUSE_MOTION`). Outside a mouse mode, horizontal wheel is a `Noop` (no scrollback, no alt-screen arrow translation). No new `[mouse]` config keys.

Reference spec: `docs/specs/2026-06-28-mouse-wheel-horizontal-scroll-design.md`.

---

### Task 1: Engine — horizontal wheel routing + encoding

**Files:**
- Modify: `crates/orzma_tty_engine/src/wheel.rs`
- Test: `crates/orzma_tty_engine/src/wheel.rs` (inline `#[cfg(test)] mod route_tests`)

**Interfaces:**
- Consumes: existing `encode_wheel_report(modes, direction, mods, cell)`, `WheelConfig`, `WheelModifiers`, `CellCoord`, `alacritty_terminal::term::TermMode`.
- Produces (used by Task 3):
  - `WheelDir::Left`, `WheelDir::Right` enum variants.
  - `pub fn WheelAction::route_horizontal(modes: TermMode, notches: i32, mouse_cell: CellCoord, mods: WheelModifiers, cfg: &WheelConfig) -> WheelAction` — returns `WriteToPty(bytes)` of `min(|notches|, cap)` SGR/X10 reports (`cb 66` for `notches<0`, `cb 67` for `notches>0`) when any `MOUSE_MODE` bit is set; `Noop` otherwise or when `notches==0`.

- [ ] **Step 1: Write the failing tests**

Add these tests inside the existing `#[cfg(test)] mod route_tests { … }` block in `crates/orzma_tty_engine/src/wheel.rs` (it already defines `cfg_default()` and `cell()` helpers):

```rust
    #[test]
    fn horizontal_mouse_mode_emits_left_and_right() {
        let modes = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let at = CellCoord { col: 43, row: 11 };
        let left = WheelAction::route_horizontal(modes, -1, at, WheelModifiers::default(), &cfg_default());
        assert_eq!(left, WheelAction::WriteToPty(b"\x1b[<66;43;11M".to_vec()));
        let right = WheelAction::route_horizontal(modes, 1, at, WheelModifiers::default(), &cfg_default());
        assert_eq!(right, WheelAction::WriteToPty(b"\x1b[<67;43;11M".to_vec()));
    }

    #[test]
    fn horizontal_without_mouse_mode_is_noop() {
        // No horizontal scrollback or alt-screen translation: normal AND alt-screen are Noop.
        assert_eq!(
            WheelAction::route_horizontal(TermMode::empty(), -2, cell(), WheelModifiers::default(), &cfg_default()),
            WheelAction::Noop
        );
        let alt = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        assert_eq!(
            WheelAction::route_horizontal(alt, 2, cell(), WheelModifiers::default(), &cfg_default()),
            WheelAction::Noop
        );
    }

    #[test]
    fn horizontal_zero_notches_is_noop() {
        let modes = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(
            WheelAction::route_horizontal(modes, 0, cell(), WheelModifiers::default(), &cfg_default()),
            WheelAction::Noop
        );
    }

    #[test]
    fn horizontal_x10_fallback_right() {
        let modes = TermMode::MOUSE_DRAG; // no SGR_MOUSE → X10
        let action = WheelAction::route_horizontal(modes, 1, CellCoord { col: 1, row: 1 }, WheelModifiers::default(), &cfg_default());
        // cb 67 + 32 = 99; col/row 1 + 32 = 33
        assert_eq!(action, WheelAction::WriteToPty(vec![0x1b, b'[', b'M', 99, 33, 33]));
    }

    #[test]
    fn horizontal_concats_and_caps() {
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let cfg = WheelConfig { max_protocol_events_per_frame: 4, ..WheelConfig::default() };
        let action = WheelAction::route_horizontal(modes, 20, CellCoord { col: 1, row: 1 }, WheelModifiers::default(), &cfg);
        let one = b"\x1b[<67;1;1M";
        let mut expected = Vec::new();
        for _ in 0..4 {
            expected.extend_from_slice(one);
        }
        assert_eq!(action, WheelAction::WriteToPty(expected));
    }

    #[test]
    fn horizontal_zero_cap_is_noop() {
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let cfg = WheelConfig { max_protocol_events_per_frame: 0, ..WheelConfig::default() };
        assert_eq!(
            WheelAction::route_horizontal(modes, 5, cell(), WheelModifiers::default(), &cfg),
            WheelAction::Noop
        );
    }

    #[test]
    fn horizontal_ctrl_modifier_bit() {
        let modes = TermMode::SGR_MOUSE | TermMode::MOUSE_REPORT_CLICK;
        let mods = WheelModifiers { ctrl: true, ..Default::default() };
        let action = WheelAction::route_horizontal(modes, -1, CellCoord { col: 1, row: 1 }, mods, &cfg_default());
        // 66 + 16 (ctrl) = 82
        assert_eq!(action, WheelAction::WriteToPty(b"\x1b[<82;1;1M".to_vec()));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p orzma_tty_engine route_tests::horizontal`
Expected: FAIL — compile error `no variant or associated item named route_horizontal` / `no variant named Left` for `WheelDir`.

- [ ] **Step 3: Implement the engine changes**

In `crates/orzma_tty_engine/src/wheel.rs`:

**(a)** Extend `WheelDir` (replace the existing enum, dropping the "out of scope" doc):

```rust
/// Wheel direction (vertical and horizontal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelDir {
    Up,
    Down,
    Left,
    Right,
}
```

**(b)** Add the `cb_base` arms in `encode_wheel_report` (the `match direction` block):

```rust
    let cb_base: u8 = match direction {
        WheelDir::Up => 64,
        WheelDir::Down => 65,
        WheelDir::Left => 66,
        WheelDir::Right => 67,
    };
```

**(c)** Replace the mouse-protocol block inside `WheelAction::route` (the `if any_mouse { … }` body) with a call to a shared helper:

```rust
        if any_mouse {
            return emit_protocol_reports(modes, direction, notches, mouse_cell, mods, cfg);
        }
```

**(d)** Add the shared private helper (place it next to `encode_wheel_report`, above `impl WheelAction`):

```rust
/// Emits `min(|notches|, cap)` concatenated wheel reports for a mouse-mode
/// pane, or `Noop` when the cap rounds the count to zero. Shared by the
/// vertical (`route`) and horizontal (`route_horizontal`) mouse-protocol paths.
fn emit_protocol_reports(
    modes: TermMode,
    direction: WheelDir,
    notches: i32,
    mouse_cell: CellCoord,
    mods: WheelModifiers,
    cfg: &WheelConfig,
) -> WheelAction {
    let count = notches.unsigned_abs().min(cfg.max_protocol_events_per_frame);
    if count == 0 {
        return WheelAction::Noop;
    }
    let mut buf = Vec::new();
    for _ in 0..count {
        buf.extend_from_slice(&encode_wheel_report(modes, direction, mods, mouse_cell));
    }
    WheelAction::WriteToPty(buf)
}
```

**(e)** Add `route_horizontal` inside `impl WheelAction`, immediately after `route` (both `pub`, surface-first ordering preserved):

```rust
    /// Decides what to do with a horizontal wheel input.
    ///
    /// `notches` is sign-significant (negative = left, positive = right).
    /// Horizontal wheel only has meaning for mouse-mode applications: when any of
    /// `MOUSE_REPORT_CLICK`, `MOUSE_DRAG`, `MOUSE_MOTION` is set, it emits
    /// `min(|notches|, max_protocol_events_per_frame)` SGR/X10 reports with `cb`
    /// 66 (left) / 67 (right). Outside a mouse mode there is no horizontal
    /// scrollback or alt-screen translation, so it returns `Noop`.
    pub fn route_horizontal(
        modes: TermMode,
        notches: i32,
        mouse_cell: CellCoord,
        mods: WheelModifiers,
        cfg: &WheelConfig,
    ) -> Self {
        if notches == 0 {
            return WheelAction::Noop;
        }
        let direction = if notches < 0 {
            WheelDir::Left
        } else {
            WheelDir::Right
        };
        if modes.intersects(
            TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION,
        ) {
            return emit_protocol_reports(modes, direction, notches, mouse_cell, mods, cfg);
        }
        WheelAction::Noop
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p orzma_tty_engine`
Expected: PASS — all new `horizontal_*` tests pass AND every pre-existing `route_tests` / `sgr_tests` / `x10_tests` / `alt_screen_tests` test still passes (the `route` refactor is behavior-preserving).

- [ ] **Step 5: Commit**

```bash
git add crates/orzma_tty_engine/src/wheel.rs
git commit -m "feat(tty-engine): horizontal wheel routing (SGR/X10 cb 66/67)"
```

---

### Task 2: Host — per-axis accumulator refactor (no behavior change)

**Files:**
- Modify: `crates/orzma_terminal/src/mouse.rs`
- Test: `crates/orzma_terminal/src/mouse.rs` (existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces (used by Task 3):
  - `accumulate_notches(residual: &mut f32, delta_cells: f32, cells_per_notch: f32) -> i32` — now takes the residual by `&mut f32` so either axis can drive it. (`WheelAccumulator` itself is unchanged this task; the horizontal field is added in Task 3, where it is first used.)
- Consumes: nothing new. This is a pure refactor: `cargo test -p orzma_terminal` must stay green with identical behavior.

- [ ] **Step 1: Change `accumulate_notches` to take `&mut f32`**

In `crates/orzma_terminal/src/mouse.rs`, replace `accumulate_notches` to take the residual by `&mut f32`. The `WheelAccumulator` struct is **unchanged** this task — the horizontal field is added in Task 3, where it is first used:

```rust
/// Adds `delta_cells` to `residual` and returns whole notches to emit
/// (positive = up/older for the vertical axis, right for the horizontal axis),
/// carrying the remainder. Resets `residual` on a sign flip, then processes the
/// new delta at full magnitude. A zero / negative-zero delta has no direction
/// and must not trip the sign-flip reset.
pub(crate) fn accumulate_notches(residual: &mut f32, delta_cells: f32, cells_per_notch: f32) -> i32 {
    if *residual != 0.0 && delta_cells != 0.0 && (*residual).signum() != delta_cells.signum() {
        *residual = 0.0;
    }
    let threshold = cells_per_notch.max(f32::EPSILON);
    *residual += delta_cells;
    let notches = (*residual / threshold).trunc() as i32;
    if notches != 0 {
        *residual -= notches as f32 * threshold;
    }
    notches
}
```

Update the **one** existing call site in `dispatch_mouse_wheel` (vertical path) to pass the field — minimal change to keep compiling, no behavior change:

```rust
    let raw = accumulate_notches(&mut gesture_acc.residual_cells, delta_cells, cfg.cells_per_notch);
```

- [ ] **Step 2: Update the three existing accumulator tests to the new signature**

In the `#[cfg(test)] mod tests` block, update these three tests to pass `&mut acc.residual_cells`:

```rust
    #[test]
    fn accumulator_emits_on_threshold_and_carries_remainder() {
        let mut acc = WheelAccumulator::default();
        assert_eq!(accumulate_notches(&mut acc.residual_cells, 0.3, 0.5), 0);
        assert_eq!(accumulate_notches(&mut acc.residual_cells, 0.3, 0.5), 1);
        assert_eq!(accumulate_notches(&mut acc.residual_cells, -1.0, 0.5), -2);
    }

    #[test]
    fn wheel_accumulator_resets_residual_on_target_change() {
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let mut acc = WheelAccumulator::default();
        acc.retarget(a);
        assert_eq!(accumulate_notches(&mut acc.residual_cells, 0.3, 0.5), 0);
        acc.retarget(a);
        assert_eq!(
            accumulate_notches(&mut acc.residual_cells, 0.3, 0.5),
            1,
            "0.3 + 0.3 = 0.6 → one notch on the same target"
        );
        acc.retarget(b);
        assert_eq!(
            accumulate_notches(&mut acc.residual_cells, 0.3, 0.5),
            0,
            "switching target clears the carried residual"
        );
    }

    #[test]
    fn accumulator_zero_delta_does_not_reset_residual() {
        // A zero / negative-zero delta has no direction and must NOT trip the
        // sign-flip reset (signum(-0.0) == -1.0 would otherwise drop the carry).
        let mut acc = WheelAccumulator::default();
        assert_eq!(accumulate_notches(&mut acc.residual_cells, 0.3, 0.5), 0);
        assert_eq!(accumulate_notches(&mut acc.residual_cells, -0.0, 0.5), 0);
        assert_eq!(accumulate_notches(&mut acc.residual_cells, 0.3, 0.5), 1);
    }
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p orzma_terminal`
Expected: PASS — all tests green, behavior unchanged (the new `residual_cells_h` field is unused this task).

- [ ] **Step 4: Commit**

```bash
git add crates/orzma_terminal/src/mouse.rs
git commit -m "refactor(terminal): per-axis wheel accumulator (accumulate_notches takes &mut f32)"
```

---

### Task 3: Host — horizontal wheel dispatch

**Files:**
- Modify: `crates/orzma_terminal/src/mouse.rs`
- Test: `crates/orzma_terminal/src/mouse.rs` (existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes:
  - `WheelAction::route_horizontal(modes, notches, mouse_cell, mods, cfg)` (Task 1).
  - `accumulate_notches(&mut f32, …)` (Task 2). This task adds the `WheelAccumulator.residual_cells_h` field it consumes.
  - Existing `decide_wheel`, `build_wheel_modifiers`, `wheel_delta_cells`, `CellContext { cell_w, cell_h, … }`, `TerminalMouseEffects`.
- Produces:
  - `fn effects_from_wheel_action(action: WheelAction) -> Vec<MouseEffect>` (private) — the shared `WheelAction → effects` mapping.
  - `fn build_wheel_modifiers_horizontal(keys: &ButtonInput<KeyCode>, cfg: &OrzmaMouseConfig) -> WheelModifiers` (private) — strips Shift on macOS.
  - Updated `dispatch_mouse_wheel` emitting both axes in one merged `TerminalMouseEffects` trigger.

- [ ] **Step 1: Write the helper tests**

In the `#[cfg(test)] mod tests` block, add:

```rust
    #[test]
    fn effects_from_wheel_action_maps_each_variant() {
        assert_eq!(effects_from_wheel_action(WheelAction::Noop), vec![]);
        assert_eq!(
            effects_from_wheel_action(WheelAction::WriteToPty(b"x".to_vec())),
            vec![MouseEffect::Write(b"x".to_vec())]
        );
        assert_eq!(
            effects_from_wheel_action(WheelAction::ScrollViewport(3)),
            vec![MouseEffect::Scroll(3)]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn horizontal_modifiers_strip_shift_on_macos() {
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ShiftLeft);
        let cfg = OrzmaMouseConfig::default();
        let mods = build_wheel_modifiers_horizontal(&keys, &cfg);
        assert!(
            !mods.shift,
            "macOS converts Shift+wheel to horizontal at the OS level; the report must not carry the Shift bit"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p orzma_terminal effects_from_wheel_action_maps_each_variant`
Expected: FAIL — compile error: `effects_from_wheel_action` / `build_wheel_modifiers_horizontal` not found.

- [ ] **Step 3: Implement the two helpers and route `decide_wheel` through the mapper**

In `crates/orzma_terminal/src/mouse.rs`:

Replace `decide_wheel` so the `WheelAction → effects` mapping lives in one shared helper:

```rust
/// Pure wheel decision. `notches` is in the engine convention (negative =
/// up/older); callers negate the Bevy-derived up-positive value before calling.
pub(crate) fn decide_wheel(
    modes: TermMode,
    notches: i32,
    cell: CellCoord,
    mods: WheelModifiers,
    cfg: &WheelConfig,
) -> Vec<MouseEffect> {
    effects_from_wheel_action(WheelAction::route(modes, notches, cell, mods, cfg))
}

/// Maps a routed `WheelAction` to host effects. Shared by the vertical
/// (`route`) and horizontal (`route_horizontal`) wheel paths.
fn effects_from_wheel_action(action: WheelAction) -> Vec<MouseEffect> {
    match action {
        WheelAction::Noop => Vec::new(),
        WheelAction::WriteToPty(b) => vec![MouseEffect::Write(b)],
        WheelAction::ScrollViewport(lines) => vec![MouseEffect::Scroll(lines)],
    }
}
```

Add `build_wheel_modifiers_horizontal` next to the existing `build_wheel_modifiers`:

```rust
/// Horizontal-wheel modifiers. On macOS the OS converts Shift+wheel into a
/// horizontal scroll while Shift stays physically held; stripping the Shift bit
/// keeps the report a plain `<ScrollWheelLeft/Right>` rather than the shifted
/// (and by default unmapped) variant. Other platforms pass modifiers through.
fn build_wheel_modifiers_horizontal(keys: &ButtonInput<KeyCode>, cfg: &OrzmaMouseConfig) -> WheelModifiers {
    let mut mods = build_wheel_modifiers(keys, cfg);
    if cfg!(target_os = "macos") {
        mods.shift = false;
    }
    mods
}
```

- [ ] **Step 4: Run the helper tests to verify they pass**

Run: `cargo test -p orzma_terminal effects_from_wheel_action_maps_each_variant horizontal_modifiers_strip_shift_on_macos`
Expected: PASS.

- [ ] **Step 5: Commit the helpers**

```bash
git add crates/orzma_terminal/src/mouse.rs
git commit -m "feat(terminal): wheel effect mapper + macOS horizontal modifier strip"
```

- [ ] **Step 6: Write the dispatch integration tests + harness**

Add a wheel test harness and the dispatch tests to the `#[cfg(test)] mod tests` block. (`CapturedEffects`, `test_metrics`, `set_phys_cursor` already exist in this module.)

```rust
    fn make_wheel_app(enable_modes: &[u8]) -> App {
        use bevy::window::WindowResolution;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseWheel>()
            .init_resource::<OrzmaMouseConfig>()
            .init_resource::<WheelAccumulator>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Clipboard>()
            .init_resource::<CapturedEffects>()
            .insert_resource(test_metrics())
            .add_observer(
                |ev: On<TerminalMouseEffects>, mut cap: ResMut<CapturedEffects>| {
                    cap.0.push(ev.effects.clone());
                },
            )
            .add_systems(Update, dispatch_mouse_wheel);

        let mut handle = TerminalHandle::detached(100, 37);
        // `\x1b[?1000;1006h` = enable X10 mouse reporting (?1000) + SGR ext (?1006).
        handle.advance(enable_modes);
        app.world_mut().spawn((
            OrzmaTerminal,
            handle,
            ComputedNode {
                size: Vec2::new(800.0, 600.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(400.0, 300.0),
            TerminalGrid {
                cols: 100,
                rows: 37,
                ..default()
            },
        ));
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        app
    }

    fn write_wheel(app: &mut App, x: f32, y: f32) {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseWheel>>()
            .write(MouseWheel {
                unit: MouseScrollUnit::Line,
                x,
                y,
                window: Entity::PLACEHOLDER,
            });
    }

    #[test]
    fn dispatch_pure_horizontal_right_emits_sgr_67() {
        // Pure horizontal (y = 0): also guards the regression where the vertical
        // `raw == 0` early-return would drop horizontal-only frames.
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5, 0.0);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0.iter().flatten().any(
                |e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<67;"))
            ),
            "a +x wheel in mouse mode must emit an SGR wheel-right (cb 67) report, got {:?}",
            cap.0
        );
    }

    #[test]
    fn dispatch_horizontal_left_emits_sgr_66() {
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, -0.5, 0.0);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0.iter().flatten().any(
                |e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<66;"))
            ),
            "a -x wheel in mouse mode must emit an SGR wheel-left (cb 66) report, got {:?}",
            cap.0
        );
    }

    #[test]
    fn dispatch_diagonal_emits_both_axes_in_one_trigger() {
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        // +x → right (67); -y is Bevy "up/older" → negated → engine down (cb 65).
        write_wheel(&mut app, 0.5, -0.5);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert_eq!(cap.0.len(), 1, "both axes must arrive in ONE trigger, got {:?}", cap.0);
        let frame = &cap.0[0];
        assert!(
            frame.iter().any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<65;"))),
            "vertical (down, cb 65) report missing: {frame:?}"
        );
        assert!(
            frame.iter().any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<67;"))),
            "horizontal (right, cb 67) report missing: {frame:?}"
        );
    }

    #[test]
    fn dispatch_horizontal_without_mouse_mode_emits_no_report() {
        let mut app = make_wheel_app(b""); // no mouse mode enabled
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5, 0.0);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0.iter().flatten().all(|e| !matches!(e, MouseEffect::Write(_))),
            "horizontal wheel outside a mouse mode must not emit a report, got {:?}",
            cap.0
        );
    }
```

- [ ] **Step 7: Run to verify the dispatch tests fail**

Run: `cargo test -p orzma_terminal dispatch_pure_horizontal_right_emits_sgr_67 dispatch_horizontal_left_emits_sgr_66 dispatch_diagonal_emits_both_axes_in_one_trigger`
Expected: FAIL — `dispatch_mouse_wheel` still reads only `ev.y`, so no `cb 66/67` report is produced (the right/left/diagonal asserts fail). `dispatch_horizontal_without_mouse_mode_emits_no_report` may already pass.

- [ ] **Step 8: Add the horizontal residual field, then rewrite `dispatch_mouse_wheel`**

First add the horizontal residual to `WheelAccumulator` and zero it on retarget. Replace the struct + its `retarget` impl:

```rust
/// Carries the sub-notch wheel remainder across frames, per axis, scoped to the
/// last terminal the wheel targeted.
#[derive(Resource, Default)]
pub(crate) struct WheelAccumulator {
    residual_cells: f32,
    residual_cells_h: f32,
    last_target: Option<Entity>,
}

impl WheelAccumulator {
    /// Resets both residuals when the wheel target changes, so a sub-notch
    /// fraction accumulated over one terminal cannot bleed into the next.
    fn retarget(&mut self, entity: Entity) {
        if self.last_target != Some(entity) {
            self.residual_cells = 0.0;
            self.residual_cells_h = 0.0;
            self.last_target = Some(entity);
        }
    }
}
```

Then replace the tail of `dispatch_mouse_wheel` — from `gesture_acc.retarget(target);` through the final `if !effects.is_empty() { … }` block — with:

```rust
    gesture_acc.retarget(target);
    let (delta_v, delta_h) = wheel.read().fold((0.0f32, 0.0f32), |(v, h), ev| {
        (
            v + wheel_delta_cells(ev.unit, ev.y, ctx.cell_h),
            h + wheel_delta_cells(ev.unit, ev.x, ctx.cell_w),
        )
    });
    let raw_v = accumulate_notches(&mut gesture_acc.residual_cells, delta_v, cfg.cells_per_notch);
    let raw_h = accumulate_notches(&mut gesture_acc.residual_cells_h, delta_h, cfg.cells_per_notch);
    if raw_v == 0 && raw_h == 0 {
        return;
    }
    let cell = ctx
        .hit(cursor_phys)
        .map(|(cell, _)| cell)
        .unwrap_or(CellCoord { col: 1, row: 1 });
    let modes = handle.current_modes();
    let mut effects = Vec::new();
    if raw_v != 0 {
        // NOTE: Bevy +y (up/older) → engine convention (negative = up/older).
        let mods = build_wheel_modifiers(&keys, &cfg);
        effects.extend(decide_wheel(modes, -raw_v, cell, mods, &cfg.wheel));
    }
    if raw_h != 0 {
        // NOTE: positive ev.x → Right (cb 67); confirm against a live Neovim and
        // flip to `-raw_h` if reversed (winit's macOS PixelDelta horizontal sign
        // is historically opposite X11/Wayland, so the on-screen direction is
        // runtime-verified, not assumed).
        let mods = build_wheel_modifiers_horizontal(&keys, &cfg);
        effects.extend(effects_from_wheel_action(WheelAction::route_horizontal(
            modes, raw_h, cell, mods, &cfg.wheel,
        )));
    }
    if !effects.is_empty() {
        commands.trigger(TerminalMouseEffects {
            entity: target,
            effects,
        });
    }
```

- [ ] **Step 9: Run the full crate test suite to verify everything passes**

Run: `cargo test -p orzma_terminal`
Expected: PASS — the four `dispatch_*` tests pass and every pre-existing test still passes.

- [ ] **Step 10: Commit**

```bash
git add crates/orzma_terminal/src/mouse.rs
git commit -m "feat(terminal): forward horizontal wheel to mouse-mode apps (SGR 66/67)"
```

---

### Task 4: Workspace verification + manual direction check

**Files:**
- None (verification only).

**Interfaces:**
- Consumes: the completed Tasks 1–3.

- [ ] **Step 1: Run both crates' tests**

Run: `cargo test -p orzma_tty_engine -p orzma_terminal`
Expected: PASS — no failures.

- [ ] **Step 2: Lint and format**

Run: `cargo clippy -p orzma_tty_engine -p orzma_terminal --all-targets && cargo fmt`
Expected: no clippy warnings; `cargo fmt` leaves no diff (or run `git diff --exit-code` after to confirm).

- [ ] **Step 3: Confirm the workspace still builds**

Run: `cargo build -p orzma_tty_engine -p orzma_terminal`
Expected: clean build.

- [ ] **Step 4: Manual direction verification (human-in-the-loop)**

The on-screen direction is the one thing unit tests cannot confirm (`ev.x` sign + `cb` assignment compose into a perceived direction; winit's macOS horizontal sign is historically flipped). Hand this checklist to the user:

1. `cargo run` (or `just run`).
2. In a pane, run Neovim with `:set mouse=a` and `:set nowrap`, open a file with long lines.
3. **Trackpad two-finger swipe right** → the view should scroll right (reveal text to the right). Swipe left → scroll left.
4. If the direction is **reversed**, flip the horizontal sign: change `route_horizontal(modes, raw_h, …)` to `route_horizontal(modes, -raw_h, …)` in `dispatch_mouse_wheel` (and update the `// NOTE:` accordingly), then re-run.
5. **macOS only:** with a traditional mouse, **Shift+scroll-wheel** should also scroll horizontally (plain `<ScrollWheelLeft/Right>`, not `<S-…>`).
6. Confirm a **pure vertical** scroll still behaves exactly as before (no regression).

- [ ] **Step 5: Final commit (if the sign was flipped or any doc tweak was needed)**

```bash
git add -A
git commit -m "fix(terminal): correct horizontal wheel direction after live verification"
```

(If no change was needed in Step 4, skip this commit.)

---

## Notes for the implementer

- **Bevy `MouseWheel` fields:** the test harness writes `MouseWheel { unit, x, y, window }`. If the installed Bevy version's struct differs (e.g. no `window`), match the actual definition — the compiler will tell you.
- **Why no change to `src/mode/tmux/input.rs`:** horizontal is mouse-mode-only, and mouse-mode tmux panes are `WheelOwner::CededToOrzma`, owned by `dispatch_mouse_wheel`. `forward_wheel_to_tmux` only handles copy-mode and alt-screen-residual (no horizontal meaning), and holds an independent `MessageReader<MouseWheel>` cursor, so it needs no change.
- **Why no change to `src/mode/default/*`:** Default-mode terminal wheel is already owned by the always-on `dispatch_mouse_wheel`; the only Default wheel system is webview-only.
