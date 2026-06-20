# Dynamic Window Title Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the OS window title reflect the active context — `session:active-window — ozmux` in Ozmux mode, the focused terminal's OSC title + ` — ozmux` in Ozma mode, falling back to `ozmux`.

**Architecture:** A new private module `src/window_title.rs` exposes a `pub(crate) WindowTitlePlugin` that registers two `Update` systems, each gated by an `in_state(AppMode::…)` run condition. Each system reads the relevant components, builds the desired title with a pure formatter, and writes `window.title` only when it differs (conditional mutation drives change detection). All non-test symbols are reachable from the registered plugin, so the build stays warning-clean.

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18 ECS, `bevy::window::Window`. No new dependencies.

## Global Constraints

- Comments: English only; only `// TODO:` / `// NOTE:` / `// SAFETY:` line comments allowed; no block or narrative comments.
- No `mod.rs`; module file is `src/window_title.rs`.
- `//!` module doc required at the top of `src/window_title.rs`; `///` on the one `pub(crate)` item (`WindowTitlePlugin`). Private items need no docs.
- Item ordering: `pub(crate) WindowTitlePlugin` + its `impl Plugin` first; private consts/systems/formatters/`#[cfg(test)]` after.
- All `use` statements form a single contiguous block at the top (no blank lines between groups, no inline fully-qualified paths). `#[cfg(test)]` modules may use local `use`.
- `Plugin::build` body is a single method chain off `app`.
- Function signatures: mutable parameters before immutable (`mut window: Query<&mut Window, …>` before read-only queries).
- `Query` system params must not use a `_q` suffix; use descriptive nouns.
- Change detection: write `window.title` conditionally; never `set_changed()` / `bypass_change_detection()`.
- CI is strict: `RUSTFLAGS="-D warnings"`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo test --workspace --tests -- --test-threads=1`. Every task must end warning-clean and formatted.
- Suffix string is exactly ` — ozmux` (space, EM DASH U+2014, space, `ozmux`). Fallback string is exactly `ozmux`.

---

### Task 1: Window-title module + plugin wiring (formatters, systems, unit tests)

Creates the whole feature in one warning-clean unit: the pure formatters (with unit tests), the conditional-write apply helper, the two mode-gated systems, the `WindowTitlePlugin`, and its registration in `main.rs`. Integration tests for the systems follow in Task 2.

**Files:**
- Create: `src/window_title.rs`
- Modify: `src/main.rs` (add `mod window_title;`, the import, and `.add_plugins(WindowTitlePlugin)`)
- Test: unit tests live in `src/window_title.rs` under `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes (from the existing codebase):
  - `crate::ozma::AppMode` — `pub(crate)` `States` enum, variants `Ozma` / `Ozmux`.
  - `ozma_tty_engine::TerminalTitle` — `pub struct TerminalTitle(pub Option<String>)` component.
  - `ozma_terminal::{KeyboardFocused, OzmaTerminal}` — unit-struct marker components.
  - `ozmux_tmux::{ActiveWindow, TmuxSession, TmuxWindow}` — `TmuxSession { id, name: String }`, `TmuxWindow { id, index, name: String }`, `ActiveWindow` marker.
  - `bevy::window::{PrimaryWindow, Window}` — `Window.title: String`.
- Produces (used by Task 2 and `main.rs`):
  - `pub(crate) struct WindowTitlePlugin;` implementing `bevy::prelude::Plugin`.
  - Private `fn format_ozma(title: Option<&str>) -> String`.
  - Private `fn format_ozmux(session: &str, window: Option<&str>) -> String`.
  - Private `fn apply_title(window: &mut Window, desired: String)`.
  - Private systems `fn update_ozma_window_title(...)`, `fn update_ozmux_window_title(...)`.

- [ ] **Step 1: Write the failing formatter unit tests**

Create `src/window_title.rs` with ONLY the module doc and the test module for now (the functions don't exist yet, so this fails to compile — that is the red state):

```rust
//! Dynamic OS window-title sync: reflects the active context per `AppMode`
//! into the primary window's title bar — `session:window — ozmux` in Ozmux
//! mode, the focused terminal's OSC title + ` — ozmux` in Ozma mode.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ozma_some_title_gets_suffix() {
        assert_eq!(format_ozma(Some("vim")), "vim — ozmux");
    }

    #[test]
    fn ozma_empty_title_is_app_name() {
        assert_eq!(format_ozma(Some("")), "ozmux");
    }

    #[test]
    fn ozma_none_title_is_app_name() {
        assert_eq!(format_ozma(None), "ozmux");
    }

    #[test]
    fn ozmux_session_and_window() {
        assert_eq!(format_ozmux("main", Some("vim")), "main:vim — ozmux");
    }

    #[test]
    fn ozmux_session_only_when_window_absent() {
        assert_eq!(format_ozmux("main", None), "main — ozmux");
    }

    #[test]
    fn ozmux_session_only_when_window_empty() {
        assert_eq!(format_ozmux("main", Some("")), "main — ozmux");
    }

    #[test]
    fn ozmux_empty_session_is_app_name() {
        assert_eq!(format_ozmux("", Some("vim")), "ozmux");
        assert_eq!(format_ozmux("", None), "ozmux");
    }
}
```

Also declare the module so it compiles as part of the crate. In `src/main.rs`, add `mod window_title;` immediately after `mod webview_render;`:

```rust
mod webview_render;
mod window_title;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ozmux-gui --bin ozmux-gui window_title`
Expected: FAIL — compile error `cannot find function format_ozma in this scope` (and `format_ozmux`).

- [ ] **Step 3: Implement the full module**

Replace the contents of `src/window_title.rs` with the complete module (imports, plugin, consts, systems, formatters, apply helper, and the test module from Step 1):

```rust
//! Dynamic OS window-title sync: reflects the active context per `AppMode`
//! into the primary window's title bar — `session:window — ozmux` in Ozmux
//! mode, the focused terminal's OSC title + ` — ozmux` in Ozma mode.

use crate::ozma::AppMode;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use ozma_terminal::{KeyboardFocused, OzmaTerminal};
use ozma_tty_engine::TerminalTitle;
use ozmux_tmux::{ActiveWindow, TmuxSession, TmuxWindow};

/// Keeps the primary OS window title in sync with the active `AppMode`
/// context: the tmux `session:window` in Ozmux mode, and the focused
/// terminal's OSC title in Ozma mode.
pub(crate) struct WindowTitlePlugin;

impl Plugin for WindowTitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                update_ozma_window_title.run_if(in_state(AppMode::Ozma)),
                update_ozmux_window_title.run_if(in_state(AppMode::Ozmux)),
            ),
        );
    }
}

const APP_NAME: &str = "ozmux";

const SUFFIX: &str = " — ozmux";

fn update_ozma_window_title(
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    focused: Query<&TerminalTitle, (With<OzmaTerminal>, With<KeyboardFocused>)>,
) {
    let Ok(mut window) = window.single_mut() else {
        return;
    };
    let Ok(title) = focused.single() else {
        return;
    };
    apply_title(&mut window, format_ozma(title.0.as_deref()));
}

fn update_ozmux_window_title(
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    sessions: Query<&TmuxSession>,
    active_windows: Query<&TmuxWindow, With<ActiveWindow>>,
) {
    let Ok(mut window) = window.single_mut() else {
        return;
    };
    let session = sessions
        .iter()
        .next()
        .map(|s| s.name.as_str())
        .unwrap_or("");
    let active = active_windows.iter().next().map(|w| w.name.as_str());
    apply_title(&mut window, format_ozmux(session, active));
}

fn format_ozma(title: Option<&str>) -> String {
    match title {
        Some(t) if !t.is_empty() => format!("{t}{SUFFIX}"),
        _ => APP_NAME.to_string(),
    }
}

fn format_ozmux(session: &str, window: Option<&str>) -> String {
    if session.is_empty() {
        return APP_NAME.to_string();
    }
    match window {
        Some(w) if !w.is_empty() => format!("{session}:{w}{SUFFIX}"),
        _ => format!("{session}{SUFFIX}"),
    }
}

fn apply_title(window: &mut Window, desired: String) {
    if window.title != desired {
        window.title = desired;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ozma_some_title_gets_suffix() {
        assert_eq!(format_ozma(Some("vim")), "vim — ozmux");
    }

    #[test]
    fn ozma_empty_title_is_app_name() {
        assert_eq!(format_ozma(Some("")), "ozmux");
    }

    #[test]
    fn ozma_none_title_is_app_name() {
        assert_eq!(format_ozma(None), "ozmux");
    }

    #[test]
    fn ozmux_session_and_window() {
        assert_eq!(format_ozmux("main", Some("vim")), "main:vim — ozmux");
    }

    #[test]
    fn ozmux_session_only_when_window_absent() {
        assert_eq!(format_ozmux("main", None), "main — ozmux");
    }

    #[test]
    fn ozmux_session_only_when_window_empty() {
        assert_eq!(format_ozmux("main", Some("")), "main — ozmux");
    }

    #[test]
    fn ozmux_empty_session_is_app_name() {
        assert_eq!(format_ozmux("", Some("vim")), "ozmux");
        assert_eq!(format_ozmux("", None), "ozmux");
    }
}
```

- [ ] **Step 4: Register the plugin in `main.rs`**

In `src/main.rs`, add the import next to the other `use crate::…` lines — immediately after `use crate::webview_render::{OzmuxWebviewRenderPlugin, cef_plugin};`:

```rust
use crate::webview_render::{OzmuxWebviewRenderPlugin, cef_plugin};
use crate::window_title::WindowTitlePlugin;
```

Then register the plugin as its own `add_plugins` call (kept standalone to avoid growing the 14-element plugin tuple). Find:

```rust
        .add_plugins(RenamePromptPlugin)
        .add_plugins((
```

and change it to:

```rust
        .add_plugins(RenamePromptPlugin)
        .add_plugins(WindowTitlePlugin)
        .add_plugins((
```

- [ ] **Step 5: Run the unit tests to verify they pass**

Run: `cargo test -p ozmux-gui --bin ozmux-gui window_title`
Expected: PASS — 7 tests pass (`ozma_*`, `ozmux_*`).

- [ ] **Step 6: Verify the build is warning-clean and formatted**

Run: `cargo fmt -p ozmux-gui`
Run: `cargo clippy -p ozmux-gui --all-targets -- -D warnings`
Expected: no warnings, no errors. (Confirms `format_*`, `apply_title`, both systems, and the plugin all have non-test callers — no `dead_code`.)

- [ ] **Step 7: Commit**

```bash
git add src/window_title.rs src/main.rs
git commit -m "$(cat <<'EOF'
feat(window_title): dynamic OS window title per AppMode

Reflect the active context into the primary window title: tmux
session:window in Ozmux mode, the focused terminal's OSC title in Ozma
mode, falling back to "ozmux". Two in_state-gated Update systems write
window.title conditionally via a pure formatter.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Integration tests for the two mode systems

Adds one integration test per system to the existing `#[cfg(test)] mod tests` in `src/window_title.rs`. Each builds a minimal `App`, spawns a `(Window, PrimaryWindow)` entity (the systems query `&mut Window, With<PrimaryWindow>`, and `MinimalPlugins` creates no window — mirrors the `src/font.rs` test that spawns `(Window, PrimaryWindow)`), sets the `AppMode` state, spawns the mode components, runs the schedule, and asserts the primary window's `.title`.

**Files:**
- Modify/Test: `src/window_title.rs` (extend the `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes (from Task 1): `WindowTitlePlugin`, and the components re-exported at the module top (`Window`, `PrimaryWindow`, `OzmaTerminal`, `KeyboardFocused`, `TerminalTitle`, `TmuxSession`, `TmuxWindow`, `ActiveWindow`, `AppMode`).
- Consumes (new test-only imports): `bevy::state::app::StatesPlugin`, `ozmux_tmux::{SessionId, WindowId}`.
- Produces: nothing new — pure test coverage.

- [ ] **Step 1: Write the integration tests**

In `src/window_title.rs`, inside `#[cfg(test)] mod tests`, add the test-only imports right after `use super::*;` and append the two tests and the helper. The `mod tests` block opens as:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::StatesPlugin;
    use ozmux_tmux::{SessionId, WindowId};
```

Then add (after the existing formatter unit tests, still inside `mod tests`):

```rust
    fn primary_window_title(app: &mut App) -> String {
        let world = app.world_mut();
        let mut windows = world.query_filtered::<&Window, With<PrimaryWindow>>();
        windows
            .iter(world)
            .next()
            .expect("primary window exists")
            .title
            .clone()
    }

    #[test]
    fn ozma_system_sets_focused_terminal_title() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Ozma);
        app.add_plugins(WindowTitlePlugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.world_mut().spawn((
            OzmaTerminal,
            KeyboardFocused,
            TerminalTitle(Some("vim".to_string())),
        ));

        app.update();

        assert_eq!(primary_window_title(&mut app), "vim — ozmux");
    }

    #[test]
    fn ozmux_system_sets_session_and_active_window() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Ozmux);
        app.add_plugins(WindowTitlePlugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.world_mut().spawn(TmuxSession {
            id: SessionId(1),
            name: "main".to_string(),
        });
        app.world_mut().spawn((
            TmuxWindow {
                id: WindowId(2),
                index: 1,
                name: "vim".to_string(),
            },
            ActiveWindow,
        ));

        app.update();

        assert_eq!(primary_window_title(&mut app), "main:vim — ozmux");
    }
```

- [ ] **Step 2: Run the integration tests to verify they pass**

Run: `cargo test -p ozmux-gui --bin ozmux-gui window_title`
Expected: PASS — now 9 tests (7 formatter unit tests + `ozma_system_sets_focused_terminal_title` + `ozmux_system_sets_session_and_active_window`).

If `single_mut()` / `insert_state` API shapes differ from expectation, cross-check against `src/input/ime.rs:162,175` (`Query<&mut Window, With<PrimaryWindow>>` + `let Ok(mut window) = …single_mut() else`) and `src/tmux/dialog.rs:105-106` (`StatesPlugin` + `insert_state(AppMode::Ozmux)`); both are the live patterns this mirrors.

- [ ] **Step 3: Verify warning-clean and formatted**

Run: `cargo fmt -p ozmux-gui`
Run: `cargo clippy -p ozmux-gui --all-targets -- -D warnings`
Expected: no warnings, no errors.

- [ ] **Step 4: Commit**

```bash
git add src/window_title.rs
git commit -m "$(cat <<'EOF'
test(window_title): integration tests for Ozma/Ozmux title systems

Spawn a (Window, PrimaryWindow) entity, set AppMode, spawn the mode
components, run the schedule, and assert window.title for both modes.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final verification (after both tasks)

- [ ] Run the full workspace test + lint gate the way CI does:
  - `cargo test --workspace --tests -- --test-threads=1` → all pass.
  - `cargo clippy --workspace --all-targets -- -D warnings` → clean.
  - `cargo fmt --check` → clean.
- [ ] Manual smoke (optional, needs CEF via `make setup-cef`): `cargo run`, confirm the title shows `ozmux` at boot, then `session:window — ozmux` once attached to tmux, and the focused terminal's title + ` — ozmux` in Ozma mode.

## Spec coverage check

| Spec section | Covered by |
| --- | --- |
| Decisions table (Ozmux `session:window`, Ozma OSC title, `ozmux` fallback, ` — ozmux` suffix) | `format_ozmux` / `format_ozma` + unit tests (Task 1) |
| Module (`//!`, `pub(crate) WindowTitlePlugin`, pub-first order, private helpers) | `src/window_title.rs` structure (Task 1) |
| Pure formatters contract | Task 1 Step 3 + unit tests |
| Apply helper (conditional write) | `apply_title` (Task 1 Step 3) |
| Systems (`in_state` gating, `Query<&mut Window, With<PrimaryWindow>>`, `iter().next()` for tmux, `.single()` for focus) | `update_ozma_window_title` / `update_ozmux_window_title` (Task 1) |
| Edge cases (empty session → `ozmux`; window absent → `{session} — ozmux`; OSC `None`/empty → `ozmux`) | formatter unit tests (Task 1) |
| Transient no-focus holds last title (distinct from fallback) | `update_ozma_window_title` early-return on `single()` Err (Task 1) |
| Testing (unit + integration with `(Window, PrimaryWindow)`) | Task 1 (unit) + Task 2 (integration) |
| Static boot title stays `ozmux` | `main.rs` unchanged `title: "ozmux"`; plugin only added (Task 1 Step 4) |
| Out of scope (config templates, transient UI states, per-pane subtitles) | not implemented (intentional) |
