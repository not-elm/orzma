# Session chooser choose-tree-style redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restyle the tmux session chooser popup to a tmux `choose-tree` look — per-row `(N)` index, tree-branch glyphs, amber selection bar, footer hint, no window count — and add `j`/`k` + `Esc` navigation.

**Architecture:** Pure visual + small-input change to the existing Bevy 0.18 overlay in `src/tmux_picker.rs`, plus color/size constants in `src/theme.rs`. The Phase 4 popup card and the in-place row-update (flicker fix) are preserved; rows stay single-`Text` entities with uniform per-row color.

**Tech Stack:** Rust 2024, Bevy 0.18 UI (`Node`/`Text`/`TextColor`/`BackgroundColor`/`BorderColor`/`TextFont`), `KeyboardInput` messages.

**Spec:** `docs/superpowers/specs/2026-06-15-session-chooser-redesign-design.md`

**Conventions (`.claude/rules/rust.md`):** only `// TODO:`/`// NOTE:`/`// SAFETY:` comments; doc-comment `pub` items; all `use` at top in one block; mutable params first; private items last; no manual `set_changed`/`bypass_change_detection`. Commit messages end with:
```
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

**Test commands:** `cargo test -p ozmux-gui <name>`, `cargo build -p ozmux-gui`, `cargo clippy -p ozmux-gui --all-targets`, `cargo fmt`.

---

## File Structure

- `src/theme.rs` — add `SELECTION`, `SELECTION_FG` (Task 1), `DIVIDER`, `PICKER_TITLE_FONT_SIZE_PX` (Task 2).
- `src/tmux_picker.rs` — `row_visuals` row format + amber selection + test fix (Task 1); `spawn_picker_ui` title/footer chrome (Task 2); `handle_picker_input` build-rows-once + `j`/`k` + `Esc` (Task 3).

---

## Task 1: Row format + amber selection (`row_visuals`)

**Files:**
- Modify: `src/theme.rs` (add `SELECTION`, `SELECTION_FG`)
- Modify: `src/tmux_picker.rs` (`row_visuals`; update the `nav_reuses_row_entities_in_place` assertion)

- [ ] **Step 1: Write the failing test**

In `src/tmux_picker.rs`, inside `#[cfg(test)] mod tests`, add (the helpers `fake_session`/`fake_window` already exist; `tmux_control::SessionInfo` has public fields):

```rust
    #[test]
    fn row_visuals_choose_tree_format() {
        let picker = SessionPicker {
            sessions: vec![
                fake_session(0, "alpha"),
                SessionInfo {
                    attached: true,
                    ..fake_session(1, "beta")
                },
            ],
            windows: vec![fake_window(0, "alpha", 0, true, "zsh")],
            selected: 0,
            open: true,
            last_open: true,
        };
        let rows = build_rows(&picker.sessions, &picker.windows);
        let v = row_visuals(&picker, &rows, 0);
        // rows: (0) alpha [sel], (1) window zsh, (2) beta attached, (3) New session
        assert_eq!(v[0].0, "(0) alpha");
        assert!(!v[0].0.contains("windows"), "no window count");
        assert_eq!(v[1].0, "(1) └ 0: zsh*");
        assert_eq!(v[2].0, "(2) beta ·attached");
        assert_eq!(v[3].0, "(3) + New session");
        // selected row 0 -> amber bar + dark text
        assert_eq!(v[0].2, theme::SELECTION);
        assert_eq!(v[0].1, theme::SELECTION_FG);
        // unselected window -> transparent bar, muted text
        assert_eq!(v[1].2, Color::NONE);
        assert_eq!(v[1].1, theme::MUTED);
        // unselected session -> foreground text
        assert_eq!(v[2].1, theme::FOREGROUND);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ozmux-gui row_visuals_choose_tree_format`
Expected: FAIL — compile error (`theme::SELECTION` / `theme::SELECTION_FG` undefined).

- [ ] **Step 3: Add the theme constants**

In `src/theme.rs`, after the `ACCENT` constant (around line 27):

```rust
/// Session-chooser selection bar — tmux choose-tree style amber.
pub const SELECTION: Color = Color::srgb(0.847, 0.651, 0.341);
/// Text on the SELECTION bar — near-black for contrast.
pub const SELECTION_FG: Color = Color::srgb(0.094, 0.086, 0.063);
```

- [ ] **Step 4: Rewrite `row_visuals`**

Replace the body of `row_visuals` in `src/tmux_picker.rs` (the `.map(|(i, row)| { ... })` closure and the selection colors) with:

```rust
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let is_selected = i == selected;
            let (label, base) = match row {
                PickerRow::Session(si) => {
                    let s = &picker.sessions[*si];
                    let attached = if s.attached { " ·attached" } else { "" };
                    (format!("({}) {}{}", i, s.name, attached), theme::FOREGROUND)
                }
                PickerRow::Window { window, .. } => {
                    let w = &picker.windows[*window];
                    let active = if w.window_active { "*" } else { "" };
                    (
                        format!("({}) └ {}: {}{}", i, w.window_index, w.window_name, active),
                        theme::MUTED,
                    )
                }
                PickerRow::NewSession => (format!("({}) + New session", i), theme::MUTED),
            };
            let text_color = if is_selected { theme::SELECTION_FG } else { base };
            let bar_color = if is_selected {
                theme::SELECTION
            } else {
                Color::NONE
            };
            (label, text_color, bar_color)
        })
        .collect()
```

Also update the `row_visuals` doc comment's "accent bar + white text" wording to "amber bar + dark text" so the doc matches.

- [ ] **Step 5: Fix the preserved nav test assertion**

In `src/tmux_picker.rs`, in `nav_reuses_row_entities_in_place`, the highlight assertion currently checks `theme::ACCENT`. Change it to `theme::SELECTION`:

```rust
        let accent_rows = after
            .iter()
            .filter(|&&e| {
                app.world()
                    .get::<BackgroundColor>(e)
                    .is_some_and(|bg| bg.0 == theme::SELECTION)
            })
            .count();
        assert_eq!(accent_rows, 1, "exactly one row is highlighted after nav");
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p ozmux-gui tmux_picker`
Expected: PASS (including `row_visuals_choose_tree_format` and `nav_reuses_row_entities_in_place`).

- [ ] **Step 7: Lint + commit**

```bash
cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/theme.rs src/tmux_picker.rs
git commit -m "feat: choose-tree row format + amber selection in the session chooser

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Panel chrome — uppercase title + footer hint

**Files:**
- Modify: `src/theme.rs` (add `DIVIDER`, `PICKER_TITLE_FONT_SIZE_PX`)
- Modify: `src/tmux_picker.rs` (`spawn_picker_ui`)

NOTE: this is static UI chrome; it is verified by build + the existing
`nav_reuses_row_entities_in_place` test (which runs `spawn_picker_ui`) + manual.
No new marker component is added — the footer is a plain `Text` child.

- [ ] **Step 1: Add the theme constants**

In `src/theme.rs`, after `SELECTION_FG`:

```rust
/// Faint divider line (chooser footer separator, etc.).
pub const DIVIDER: Color = Color::srgba(1.0, 1.0, 1.0, 0.06);
/// Session-chooser title / footer font size.
pub const PICKER_TITLE_FONT_SIZE_PX: f32 = 11.0;
```

- [ ] **Step 2: Restyle the title and add the footer**

In `src/tmux_picker.rs` `spawn_picker_ui`, replace the panel's `.with_children(|panel| { ... })` block (the title `Text` and the `PickerList` node) with:

```rust
                .with_children(|panel| {
                    panel.spawn((
                        Text::new("TMUX SESSIONS"),
                        TextColor(theme::MUTED),
                        TextFont {
                            font_size: theme::PICKER_TITLE_FONT_SIZE_PX,
                            ..default()
                        },
                    ));
                    panel.spawn((
                        Node {
                            flex_direction: FlexDirection::Column,
                            width: Val::Percent(100.0),
                            row_gap: Val::Px(2.0),
                            ..default()
                        },
                        PickerList,
                    ));
                    panel.spawn((
                        Node {
                            border: UiRect::top(Val::Px(1.0)),
                            padding: UiRect::top(Val::Px(8.0)),
                            ..default()
                        },
                        BorderColor::all(theme::DIVIDER),
                        Text::new("↑↓/jk select · ⏎ open · esc cancel"),
                        TextColor(theme::MUTED),
                        TextFont {
                            font_size: theme::PICKER_TITLE_FONT_SIZE_PX,
                            ..default()
                        },
                    ));
                });
```

NOTE: `UiRect::top` exists in Bevy 0.18 (`bevy_ui-0.18.1/src/geometry.rs`); a
`Text` entity is also a `Node`, so the footer's border/padding apply to the
text node directly (same single-entity pattern the rows use).

- [ ] **Step 3: Build + run tests**

Run: `cargo build -p ozmux-gui && cargo test -p ozmux-gui tmux_picker`
Expected: compiles; `tmux_picker` tests PASS (the in-place test still finds its rows).

- [ ] **Step 4: Lint + commit**

```bash
cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/theme.rs src/tmux_picker.rs
git commit -m "feat: uppercase title + footer hint in the session chooser panel

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Input — build rows once, `j`/`k` + `Esc`

**Files:**
- Modify: `src/tmux_picker.rs` (`handle_picker_input` + a headless input test)

- [ ] **Step 1: Write the failing test**

In `src/tmux_picker.rs` `#[cfg(test)] mod tests`, add (the picker owns the
keyboard while open, so this exercises the real routing):

```rust
    fn key_press(code: KeyCode) -> bevy::input::keyboard::KeyboardInput {
        bevy::input::keyboard::KeyboardInput {
            key_code: code,
            logical_key: bevy::input::keyboard::Key::Character("x".into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }

    fn picker_input_app() -> App {
        let mut app = App::new();
        app.add_message::<bevy::input::keyboard::KeyboardInput>();
        app.insert_resource(SessionPicker {
            sessions: vec![fake_session(0, "alpha"), fake_session(1, "beta")],
            windows: vec![fake_window(0, "alpha", 0, true, "zsh")],
            selected: 0,
            open: true,
            last_open: true,
        });
        app.init_resource::<ConnectionState>();
        app.init_resource::<OzmuxConfigsResource>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_systems(Update, handle_picker_input);
        app
    }

    fn send_key(app: &mut App, code: KeyCode) {
        app.world_mut().write_message(key_press(code));
        app.update();
    }

    #[test]
    fn j_k_move_selection_like_arrows() {
        let mut app = picker_input_app();
        send_key(&mut app, KeyCode::KeyJ);
        assert_eq!(app.world().resource::<SessionPicker>().selected, 1);
        send_key(&mut app, KeyCode::KeyK);
        assert_eq!(app.world().resource::<SessionPicker>().selected, 0);
    }

    #[test]
    fn esc_closes_the_picker() {
        let mut app = picker_input_app();
        assert!(app.world().resource::<SessionPicker>().open);
        send_key(&mut app, KeyCode::Escape);
        assert!(!app.world().resource::<SessionPicker>().open);
    }
```

NOTE: if `World::write_message` is not the exact API name in this Bevy 0.18
build, use the message-send equivalent (e.g.
`app.world_mut().resource_mut::<Messages<bevy::input::keyboard::KeyboardInput>>().write(key_press(code))`).
Confirm by compiling; this is the only API uncertainty in the task.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ozmux-gui j_k_move_selection_like_arrows esc_closes_the_picker`
Expected: FAIL — `j`/`k`/`Esc` are not handled yet (selection unchanged; picker stays open).

- [ ] **Step 3: Update `handle_picker_input`**

In `src/tmux_picker.rs`, replace the body from `let entry_count = ...` through the `match ev.key_code { ... }` with (builds `rows` once, adds `j`/`k`/`Esc`):

```rust
    let rows = build_rows(&picker.sessions, &picker.windows);
    let entry_count = rows.len();
    for ev in keys.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match ev.key_code {
            KeyCode::ArrowUp | KeyCode::KeyK => {
                picker.selected = step_selection(picker.selected, entry_count, true);
            }
            KeyCode::ArrowDown | KeyCode::KeyJ => {
                picker.selected = step_selection(picker.selected, entry_count, false);
            }
            KeyCode::Escape => {
                picker.open = false;
                break;
            }
            KeyCode::Enter => {
                let row = rows
                    .get(picker.selected)
                    .copied()
                    .unwrap_or(PickerRow::NewSession);
                if connection.client().is_some() {
                    apply_switch(&mut connection, &mut state, &configs, &picker, row);
                } else {
                    apply_attach(
                        &mut connection,
                        &mut state,
                        &configs,
                        control.as_deref(),
                        &picker,
                        row,
                    );
                }
                picker.open = false;
                break;
            }
            _ => {}
        }
    }
```

(The `Enter` branch no longer calls `build_rows` again — it reuses the `rows`
built at the top.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p ozmux-gui j_k_move_selection_like_arrows esc_closes_the_picker`
Expected: PASS. Then `cargo test -p ozmux-gui tmux_picker` — all PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/tmux_picker.rs
git commit -m "feat: j/k navigation and esc-to-cancel in the session chooser

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Workspace verification (no new code)

**Files:** none

- [ ] **Step 1: Build + test the workspace**

Run: `cargo build && cargo test --workspace`
Expected: success; all tests pass (the gated `real_tmux_*` stay ignored).

- [ ] **Step 2: Lint + format check**

Run: `cargo clippy --workspace --all-targets && cargo fmt --check`
Expected: no warnings; formatting clean.

- [ ] **Step 3: Commit if fmt changed anything**

```bash
git add -A && git commit -m "chore: fmt after chooser redesign

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" || echo "nothing to commit"
```

---

## Manual verification (after all tasks)

```bash
tmux new-session -d -s alpha; tmux new-session -d -s beta
cargo run
```
- `⌘⇧P` opens the chooser. Confirm: opaque card; `TMUX SESSIONS` muted title; rows
  `(0) alpha`, `(1) └ 0: zsh*`, … with **no `(N windows)`**; amber selection bar;
  footer `↑↓/jk select · ⏎ open · esc cancel`.
- `↑↓` and `j`/`k` move the cursor (no flicker); `Esc` closes; `Enter` switches.

---

## Self-Review notes

- **Spec coverage:** row format + `(N)` + tree glyph + no window count + amber
  selection (Task 1); uppercase title + footer + theme constants `DIVIDER`/
  `PICKER_TITLE_FONT_SIZE_PX` (Task 2); `j`/`k` + `Esc` + build-rows-once
  (Task 3); preserved-test assertion fix (Task 1, Step 5). `·attached` vs `*`
  active marker covered in Task 1.
- **Out of scope (per spec):** functional digit-jump, mouse, tree collapse,
  sort modes, pane titles.
- **Type consistency:** `SELECTION`, `SELECTION_FG`, `DIVIDER`,
  `PICKER_TITLE_FONT_SIZE_PX`, `row_visuals`, `build_rows`, `step_selection`,
  `PickerRow`, `PickerRowLabel`, `SessionPicker` fields used consistently.
