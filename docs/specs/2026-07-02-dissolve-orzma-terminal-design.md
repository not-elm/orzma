# Dissolve `orzma_terminal` into `src/`

## Problem

`crates/orzma_terminal` (~1,150 lines) predates the host's input → action
architecture: `src/input/` gathers and decides, `src/action/` holds the
per-command `EntityEvent`s and their apply observers. The crate now contains
only apply-side leftovers (PTY-level action events, the `Clipboard` resource,
the `OrzmaTerminal` marker, default-mode spawn/layout/exit) that logically
belong to the binary. Keeping them in a separate library crate:

- splits the action layer across two crates (`src/action/{tmux,vi}` in the
  binary, PTY-level actions in `orzma_terminal`);
- forces `pub` visibility on items whose only consumers are the binary;
- keeps `crates/orzma_webview` coupled to a UI-layer crate solely for the
  `OrzmaTerminal` marker component.

## Goal

Delete `crates/orzma_terminal` entirely and move its contents into `src/`,
following the existing input → action structure. **Zero behavior change** —
type and event names are preserved; the bulk of the diff is `use` path
rewrites.

## Design decisions (approved)

1. **`orzma_webview` dependency severed by re-keying on `TerminalHandle`** —
   not by moving the marker into `orzma_tty_engine`, and not by keeping a
   minimal marker crate.
2. **Migrated PTY-level actions land in a new `src/action/terminal/`
   domain** — a third domain next to `tmux/` and `vi/`, one action per file,
   matching the existing pattern.
3. **The `OrzmaTerminal` marker moves to a new `src/surface.rs` and keeps its
   name** (no rename to `TerminalSurface`).

## Specification

### 1. Sever the `orzma_webview` → `orzma_terminal` dependency

The only out-of-crate dependency on `orzma_terminal` is
`crates/orzma_webview/src/control_plane.rs`, where `gc_despawned_surfaces`
uses `RemovedComponents<OrzmaTerminal>` to purge webview registrations of
despawned surfaces.

- Re-key it as `RemovedComponents<TerminalHandle>` (`orzma_tty_engine`, an
  existing dependency of `orzma_webview`). Every surface carries a
  `TerminalHandle`: tmux panes get one in `attach_tmux_pane_terminal`
  (`src/mode/tmux/render.rs`), standalone terminals via `TerminalBundle`.
  Despawns of unregistered entities are harmless no-ops
  (`remove_by_surface` returns empty; token unbinding is a no-op).
- Preserve the "must stay ungated and run every frame" invariant NOTE
  (`RemovedComponents` buffers clear at end of frame).
- Update the in-crate test that spawns `OrzmaTerminal` to spawn
  `TerminalHandle::detached(...)` instead.
- Drop `orzma_terminal` from `crates/orzma_webview/Cargo.toml`.
- Rewrite the three stale comments that reference the old keying / crate and
  are not caught by `use` rewrites: the `// NOTE:` in `src/mode/default.rs`
  (~line 89) that says the gc keys on `RemovedComponents<OrzmaTerminal>` —
  it becomes false after the re-key and must say `TerminalHandle`;
  `src/mode/tmux/render.rs:177` ("observer in `orzma_terminal`"); and
  `src/input/hyperlink.rs:12-13`.

### 2. File moves

| From (`crates/orzma_terminal/src/`) | To |
|---|---|
| `spawn.rs`: `OrzmaTerminal` marker + `on_add_inject_render` | **`src/surface.rs` (new)** — render-bundle injection fires for tmux panes too, so it is a shared surface concern; a `SurfacePlugin` registers the observer |
| `spawn.rs`: `cells_for` | `src/surface_geom.rs` (existing geometry helpers) |
| `spawn.rs`: `OrzmaTerminalBundle` / `OrzmaSpawnOptions` / `OrzmaTerminalConfig` / `resolve_shell` | `src/mode/default/spawn.rs` (new) — standalone-terminal spawning is a Default-mode concern |
| `clipboard.rs`: `Clipboard` + `build_paste_bytes` | `src/clipboard.rs` (new) — a `ClipboardPlugin` runs `init_resource::<Clipboard>`; consumers span `ui/copy_mode`, `mode/tmux/copy_mode`, `input/tmux/input`, `action/vi` |
| `action.rs`: `PasteAction` + `on_paste` | `src/action/terminal/paste.rs` |
| `mouse.rs`: apply events + observers | `src/action/terminal/`, split per action: `forward_input.rs` (type only — the routing observer already lives host-side), `mouse_write.rs`, `selection.rs` (Start/Update/Clear/Copy are tightly coupled, one file), `viewport_scroll.rs`, `open_uri.rs`. The shared backend-write helper `apply_to_terminal` (`mouse.rs:123`) lands as `pub(super)` in `src/action/terminal.rs`, next to the aggregator |
| `hyperlink.rs`: `try_open_uri` | folded into `src/action/terminal/open_uri.rs` (its only caller) |
| `exit.rs` (`AppExit` on shell exit) | `src/mode/default/exit.rs` — detached tmux panes never emit `TerminalChildExit`, but the adopted gateway keeps a real `PtyHandle` + `OrzmaTerminal`, so this observer also fires (alongside `on_gateway_child_exit`) when the gateway shell dies during tmux mode. Behavior kept as-is; add a `// NOTE:` recording this gateway coupling |
| `layout.rs` (window-fill resize) | `src/mode/default/layout.rs` — the query requires `&mut PtyHandle` + `&mut Coalescer`, so detached tmux panes never match it; after adoption the hidden gateway (which keeps `OrzmaTerminal` + `PtyHandle`) is the single match, so the system still resizes the gateway PTY during tmux mode. Behavior kept as-is; add a `// NOTE:` recording this gateway coupling |
| `lib.rs`: `OrzmaTerminalPlugin` | dissolved (see below) |

### 3. Plugin registration

Per the repo rule "systems are registered by a `Plugin` in the defining
file; parents aggregate":

- `src/action/terminal.rs` — new `TerminalActionPlugin` aggregates the
  per-file plugins; added to the existing `ActionPlugin` alongside `tmux` /
  `vi`.
- `src/mode/default.rs` — `DefaultModePlugin` gains `add_plugins` for the
  new `exit` / `layout` / `spawn` per-file plugins. `config_shell` becomes a
  field on `DefaultModePlugin`; the spawn plugin inserts
  `OrzmaTerminalConfig`.
- `src/main.rs` — drop `OrzmaTerminalPlugin { config_shell }`; add
  `SurfacePlugin` and `ClipboardPlugin`. `ClipboardPlugin` takes over the
  `init_resource::<Clipboard>` currently done by `OrzmaActionPlugin`; no other
  consumer initializes it.

### 4. Visibility and conventions

- All migrated items are demoted from `pub` to `pub(crate)` or narrower
  (they now live inside the binary). Doc comments and `//!` headers are
  preserved at the new locations.
- Comment taxonomy, import discipline, and the one-action-per-file layout
  follow `.claude/rules/rust.md`.

### 5. Cargo changes

- Remove `crates/orzma_terminal` from workspace `members`.
- Remove the `orzma_terminal` path dependency from the root `Cargo.toml` and
  `crates/orzma_webview/Cargo.toml`.
- `arboard` and `open` already exist in the root package. `anyhow` exists
  only under `[workspace.dependencies]`; add `anyhow = { workspace = true }`
  to the root `[dependencies]` (`OrzmaTerminalBundle::spawn` returns
  `anyhow::Result`).

### 6. Tests and verification

- In-crate unit tests migrate alongside their modules into the new files.
- `src/mode/tmux/render.rs` has a test registering `OrzmaTerminalPlugin`;
  replace it with the specific plugins it needs (e.g. `SurfacePlugin`).
- The `orzma_webview` control-plane test re-keys to
  `TerminalHandle::detached`.
- Verification: `cargo build` → `cargo test` → `just fix-lint`.

## Future work (not in this change)

- Replace the every-frame `gc_despawned_surfaces` system with an
  `On<Remove, TerminalHandle>` lifecycle observer, deleting the "must stay
  ungated" invariant instead of relocating it (suggested by both reviewers).
- Gate the Default-mode layout/exit systems on a `DefaultShell`-style marker
  instead of relying on query shape, once a behavior change is acceptable.
- Add an owner-surface index to `OrzmaRegistry` if dynamic registrations grow.

## Out of scope

- Renaming `OrzmaTerminal` or any event types.
- Behavior changes to layout/exit/paste/mouse semantics.
- Publishing implications: only `ratatui_orzma` is published to crates.io;
  `orzma_terminal` is internal, so deleting it is not a breaking external
  change.
