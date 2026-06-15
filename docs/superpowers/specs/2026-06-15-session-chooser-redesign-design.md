# Session chooser popup — choose-tree style redesign

Design spec — 2026-06-15
Builds on: `docs/superpowers/specs/2026-06-15-tmux-phase4-session-ux-design.md`
(the chooser landed in Phase 4; this restyles it.)

## Context

The session chooser popup (`⌘⇧P`) shipped in Phase 4 reads as cluttered
(`./popup.png`): every session row carries a `(N windows)` count, the
session/window hierarchy is weak, and the selection is a plain accent bar.
The user wants it cleaner, modeled on tmux's `choose-tree` (`./tmux.png`):
per-row quick-key indices, tree-branch glyphs, an amber selection bar, and a
footer hint — while dropping the window count. Vim-style `j`/`k` navigation and
`Esc`-to-cancel are added to match tmux's feel.

This is a visual + small-input change to one screen. The Phase 4 in-place row
update (the flicker fix) is preserved.

## Settled decisions (from brainstorming, via the visual companion)

1. **Direction: choose-tree faithful (dense), amber selection.** Per-row
   quick-key index `(N)`, tree-branch `└ ` for windows, amber highlight bar,
   footer status line. (Picked over a "clean grouped" and a "minimal flat"
   alternative.)
2. **Quick keys are display-only.** `(N)` is shown for orientation; it is the
   flat row position (0-based) in `build_rows` order, exactly like tmux. There
   is **no** digit-key jump — navigation stays `↑↓` / `j`/`k` + `Enter`.
   NOTE: tmux's bracketed index IS a functional jump key, so a tmux-literate
   user may expect `(N)` to jump; this divergence is accepted for now (wiring
   digit-jump is a later follow-up).
3. **Window count removed.** No `(N windows)` next to session names.
4. **`Esc` cancels** (closes the popup with no action).
5. **`j`/`k` move the cursor** (down/up) in addition to `↑`/`↓`.
6. **Rows stay single-Text + background** (no per-token coloring). Preserves the
   in-place update; uniform per-row color is also visually cleaner. Hierarchy
   comes from the tree glyph + indent + brighter session vs muted window color.

## Design

All changes are in `src/tmux_picker.rs` and `src/theme.rs`. Structure (backdrop
→ opaque card panel → title + `PickerList` + footer) and the in-place
`sync_picker_ui` update from Phase 4 are kept.

### Row text + color (`row_visuals`)

Each row remains one entity: `Node` (full width, padding, radius) + `Text` +
`TextColor` + `BackgroundColor` + `PickerRowLabel`. The label string now leads
with the flat index `N` (the row's position in `build_rows`):

| Row | Label format | Text color (unselected) |
|---|---|---|
| Session | `(N) <name>` + ` ·attached` if attached | `theme::FOREGROUND` |
| Window | `(N) └ <window_index>: <window_name>` + `*` if active | `theme::MUTED` |
| New session | `(N) + New session` | `theme::MUTED` |

- No `(N windows)`.
- `*` marks the active window within its session (`window_active`).
- `·attached` marks an attached session.
- Selected row (any kind): `BackgroundColor = theme::SELECTION` (amber),
  `TextColor = theme::SELECTION_FG` (dark). Unselected: `BackgroundColor =
  Color::NONE`.

`row_visuals` takes the row index `i` and uses it for the `(N)` prefix; it
already iterates with `enumerate()`, so `N == i`.

### Theme additions (`src/theme.rs`)

```rust
/// Session-chooser selection bar — tmux choose-tree style amber.
pub const SELECTION: Color = Color::srgb(0.847, 0.651, 0.341);
/// Text on the SELECTION bar — near-black for contrast.
pub const SELECTION_FG: Color = Color::srgb(0.094, 0.086, 0.063);
/// Faint divider line (chooser footer separator, etc.).
pub const DIVIDER: Color = Color::srgba(1.0, 1.0, 1.0, 0.06);
/// Session-chooser title / footer font size.
pub const PICKER_TITLE_FONT_SIZE_PX: f32 = 11.0;
```

`sync_picker_ui` swaps the selected-row colors from `theme::ACCENT` / white to
`theme::SELECTION` / `theme::SELECTION_FG`.

### Panel chrome (`spawn_picker_ui`)

- Title: keep the title node, styled muted + small + uppercased
  (`theme::MUTED`, font size `theme::PICKER_TITLE_FONT_SIZE_PX`), reading
  `TMUX SESSIONS`. NOTE: Bevy 0.18 `TextFont` has no letter-spacing/tracking —
  the "spaced" look is just the uppercased string, not a font feature.
- Footer: add one **plain `Text` child** (no marker component — it is static)
  to the panel **after** `PickerList`: `↑↓ select · ⏎ open · esc cancel`,
  `theme::MUTED`, font size `theme::PICKER_TITLE_FONT_SIZE_PX`, with a 1px top
  border (`border: UiRect::top(Val::Px(1.0))`, `BorderColor::all(theme::DIVIDER)`)
  and small top padding so it reads as a separated status line.

### Input (`handle_picker_input`)

Extend the key match (the picker already owns the keyboard while open, so these
never leak to tmux):

- `KeyCode::ArrowDown | KeyCode::KeyJ` → `step_selection(.., down)`
- `KeyCode::ArrowUp | KeyCode::KeyK` → `step_selection(.., up)`
- `KeyCode::Escape` → `picker.open = false; break;` (cancel, no action)
- `KeyCode::Enter` → unchanged (switch / attach via `apply_switch` /
  `apply_attach`).

While editing `handle_picker_input`, build the flattened `rows` once at the top
and reuse it for both `entry_count` and the `Enter` branch (today it calls
`build_rows` 2–3× per keypress).

## Affected files

- `src/tmux_picker.rs` — `row_visuals` (label format + amber selection),
  `spawn_picker_ui` (title style + footer), `handle_picker_input` (j/k, Esc).
- `src/theme.rs` — `SELECTION`, `SELECTION_FG` constants.

## Testing

- **Pure / unit:**
  - `row_visuals`: a session row label contains `(N) <name>`, has **no**
    `windows`, appends `·attached` only when attached; a window row contains
    the `└ ` glyph + `index: name` + `*` only when active; selected row gets
    `theme::SELECTION` bar + `SELECTION_FG` text; unselected gets `Color::NONE`.
  - Quick-key index equals the flat row position for every row.
- **Bevy (headless):**
  - Keep the Phase 4 `nav_reuses_row_entities_in_place` test, but UPDATE its
    selected-row assertion from `theme::ACCENT` to `theme::SELECTION` (the bar
    color changed) — otherwise the preserved test fails.
  - `j`/`k` change `selected` like `↓`/`↑`; `Esc` sets `open = false`. (Drive
    `handle_picker_input` with synthetic `KeyboardInput` messages, mirroring the
    existing input-handling test style.)
- **Manual:** `tmux new-session -d -s alpha; tmux new-session -d -s beta` then
  `cargo run` → `⌘⇧P`. Confirm: no window counts, `(N)` indices + `└` tree,
  amber selection bar, footer hint, `j`/`k` + `↑↓` move the cursor, `Esc`
  closes, `Enter` switches.

## Out of scope (unchanged)

- Functional digit-key jump (`0-9` to select) — display-only this time.
- Mouse interaction, tree collapse/expand, sort modes (tmux's `O`/`sort:`),
  pane-title display in window rows.
- The Phase 4 switch/attach/reconnect logic and the detached overlay.
