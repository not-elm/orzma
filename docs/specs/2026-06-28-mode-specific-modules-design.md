# Mode-specific modules: consolidate Default/Tmux code under `src/mode/`

## Problem

Mode-specific code (Default vs Tmux) is scattered and inconsistently organized:

- Default-mode code lives in top-level `default_`-prefixed files
  (`src/default_input.rs`, `src/default_webview.rs`) and inside
  `src/app_mode.rs` (`DefaultModePlugin` / `DefaultModeUi`).
- Tmux-mode code is a `src/tmux/` feature slice plus `src/tmux.rs`.
- `src/tmux/pane_hit.rs` holds **generic** surface geometry (`Side`,
  `phys_to_pane_local`, `cell_at_local`) that four non-tmux files import via
  `crate::tmux::pane_hit` — so Default-mode code depends on the Tmux module for
  shared helpers.

**Goal:** declare each mode's processing inside a dedicated module under
`src/mode/{default,tmux}/`, and move the shared geometry out of `tmux` so the
two mode modules are siblings with no cross-mode dependency.

## Non-goals

- **No behavior change.** Same plugins, systems, gating, ordering. Verified by
  the unchanged test suite.
- **No mode abstraction** (no `Mode` trait / mode registry) — declined during
  brainstorming.
- **No plugin-registration consolidation** — keep the existing per-plugin
  registration in `main.rs`; only update paths.
- **No cross-crate dedup** — e.g. `ozma_terminal::mouse::cell_at_local` stays;
  only the binary's geometry moves.

## Target module tree

```
src/
  mode.rs              # AppMode enum; `pub(crate) mod default; pub(crate) mod tmux;`
  mode/
    default.rs         # DefaultModeUi, DefaultModePlugin, ensure_default_mode_ui; `mod input; mod webview;`
    default/
      input.rs         # DefaultHostInputPlugin (+ tests)
      webview.rs       # DefaultWebviewPointerPlugin (+ tests)
    tmux.rs            # OzmuxTmuxPlugin, TmuxActiveSet (moved from src/tmux.rs)
    tmux/              # moved from src/tmux/ (gate, input, mouse/, render, pane_focus, …, pane_hit)
  surface_geom.rs      # NEW neutral module: Side, phys_to_pane_local, cell_at_local (+ tests)
  webview_pointer.rs   # unchanged location (mode-agnostic shared pointer core)
  input/  ui/  …       # unchanged (shared / cross-cutting)
```

This uses the Rust-2018 file-as-module form (`mode.rs` + `mode/`, `default.rs`
+ `default/`, `tmux.rs` + `tmux/`) — **not** `mod.rs`, per
`.claude/rules/rust.md`.

## File moves (use `git mv` to preserve history)

| From | To |
|---|---|
| `src/app_mode.rs` → `AppMode` enum | `src/mode.rs` |
| `src/app_mode.rs` → `DefaultModeUi` / `DefaultModePlugin` / `ensure_default_mode_ui` (+ tests) | `src/mode/default.rs` |
| `src/default_input.rs` | `src/mode/default/input.rs` |
| `src/default_webview.rs` | `src/mode/default/webview.rs` |
| `src/tmux.rs` | `src/mode/tmux.rs` |
| `src/tmux/**` | `src/mode/tmux/**` |
| `src/tmux/pane_hit.rs` → `Side`, `phys_to_pane_local`, `cell_at_local` (+ `cell_at_local` test) | `src/surface_geom.rs` (NEW) |
| `src/tmux/pane_hit.rs` → `tmux_pane_at_phys` (queries `TmuxPane`) | `src/mode/tmux/pane_hit.rs` (kept) |

`src/app_mode.rs` is deleted; its two halves move to `mode.rs` and
`mode/default.rs`.

## `src/surface_geom.rs` (extracted shared geometry)

Holds the generic, `TmuxPane`-free helpers (current visibilities preserved):

- `pub(crate) enum Side { Left, Right }`
- `pub(crate) fn phys_to_pane_local(node: &ComputedNode, transform: &UiGlobalTransform, cursor_phys_px: Vec2) -> Option<Vec2>`
- `pub(crate) fn cell_at_local(local_phys: Vec2, cell_w_phys: f32, cell_h_phys: f32, cols: u16, rows: u16) -> (u32, u32, Side)`
- the `cell_at_local` unit test moves with it.

`src/mode/tmux/pane_hit.rs` keeps `tmux_pane_at_phys`, importing `Side` /
`phys_to_pane_local` from `crate::surface_geom`. (`pane_hit.rs` currently has
only the `cell_at_local` test, which moves to `surface_geom` with the function.)

## Import-path updates (mechanical, compiler-driven)

1. `crate::app_mode::AppMode` → `crate::mode::AppMode` (~11 files:
   `input/copy_mode`, `input/ime`, `window_title`, `main`, and the moved mode
   files).
2. `crate::tmux::X` → `crate::mode::tmux::X` (within the tmux slice + `main.rs`).
3. `crate::tmux::pane_hit::{phys_to_pane_local, cell_at_local, Side}` →
   `crate::surface_geom::{…}` (in `mode/default/input`,
   `mode/default/webview`, `webview_pointer`, `input/hyperlink`, and tmux's own
   usages). `tmux_pane_at_phys` is referenced as
   `crate::mode::tmux::pane_hit::tmux_pane_at_phys`.

Doc-comment references to old paths (e.g. `crate::tmux::gate::claimed_webview_pane`,
`crate::tmux::adopt`, `crate::tmux::locale`, `crate::tmux::mouse::webview`) are
updated to the new `crate::mode::tmux::…` paths.

## Module declarations

- `src/main.rs`: replace `mod app_mode; mod tmux; mod default_input; mod
  default_webview;` with `mod mode; mod surface_geom;` (`mod webview_pointer;`
  unchanged). Update plugin imports/registrations to the new paths:
  `mode::default::{DefaultModePlugin, DefaultHostInputPlugin,
  DefaultWebviewPointerPlugin}`, `mode::tmux::OzmuxTmuxPlugin`. The plugins are
  still registered individually (no aggregator change).
- `src/mode.rs`: declares `pub(crate) mod default; pub(crate) mod tmux;` and
  defines `AppMode`.
- `src/mode/default.rs`: declares `mod input; mod webview;`, re-exports the two
  plugins (`pub(crate) use input::DefaultHostInputPlugin;` /
  `pub(crate) use webview::DefaultWebviewPointerPlugin;`) so they are reachable
  as `mode::default::…`, and holds the Default-mode UI lifecycle
  (`DefaultModePlugin` / `DefaultModeUi` / `ensure_default_mode_ui`).
- `src/mode/tmux.rs`: the existing `src/tmux.rs` body; its `mod …;` declarations
  for the tmux submodules are unchanged (relative paths).

## What does NOT move

`webview_pointer.rs`, `input/`, `ui/`, `system_set.rs`, `theme.rs`, `font.rs`,
`bootstrap.rs`, `configs.rs`, `window_title.rs`, `cef_profile.rs`. These are
shared / cross-cutting; only their `AppMode` / `pane_hit` import paths change.

## Testing & verification

Behavior-preserving refactor — **no new tests**. Verification:

- `cargo build` (every moved/renamed path resolves) + `cargo test --workspace`
  (the full existing suite — incl. the tmux mouse/wheel and Default-mode tests —
  passes unchanged).
- `cargo clippy --workspace` and `cargo fmt -- --check` clean.
- The unit-test modules that move (`cell_at_local`'s test → `surface_geom`; the
  `pane_hit` / `gate` / `default_input` / `default_webview` test modules → their
  new files) run from their new locations.

## Risks & mitigations

- **Wide import churn** (`crate::tmux::*` → `crate::mode::tmux::*`,
  `crate::app_mode::*` → `crate::mode::*`). Mitigate: compiler-driven — move,
  then fix every unresolved path until `cargo build` is green; `git mv`
  preserves blame.
- **`mod.rs` ban** (`.claude/rules/rust.md`): the new layout is the
  file-as-module form, not `mod.rs` — compliant.
- **Visibility**: items keep their current visibility; `pub(crate)` cross-module
  items (e.g. the `surface_geom` helpers) remain reachable. No widening needed.

## Implementation order

1. Create `src/surface_geom.rs`; move `Side` / `phys_to_pane_local` /
   `cell_at_local` (+ test) into it; update `pane_hit.rs` to import them; update
   the 4 consumers' imports. **Build green.**
2. `git mv src/tmux.rs src/mode/tmux.rs` and `git mv src/tmux src/mode/tmux`;
   add `src/mode.rs` with `pub(crate) mod tmux;`; fix `crate::tmux::*` →
   `crate::mode::tmux::*`. **Build green.**
3. Move `AppMode` from `app_mode.rs` into `src/mode.rs`; fix
   `crate::app_mode::AppMode` → `crate::mode::AppMode`. **Build green.**
4. Create `src/mode/default.rs` (`DefaultModePlugin` / UI from `app_mode.rs`) +
   `pub(crate) mod default;` in `mode.rs`; `git mv` `default_input.rs` →
   `mode/default/input.rs`, `default_webview.rs` → `mode/default/webview.rs`;
   delete `app_mode.rs`; fix `main.rs` `mod` decls + plugin paths. **Build green.**
5. `cargo test --workspace` + clippy + fmt. **Done.**
