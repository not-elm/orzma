# Tmux VT Scrollback on Mouse Wheel

**Date:** 2026-06-19  
**Branch:** `vt-scroll`  
**Status:** Approved

## Summary

In ozmux mode (tmux backend), mouse wheel scroll currently enters tmux copy mode
(`WheelUpPane` binding → `copy-mode -e`). This replaces that with direct scrolling
of each pane's local `TerminalHandle` VT scrollback buffer — the same approach
used by iTerm2 in tmux CC mode.

## Behavioral Specification

| Event | Before | After |
|---|---|---|
| Wheel up (not in copy mode) | tmux `WheelUpPane` → enters copy mode | Scroll local VT scrollback up |
| Wheel down (not in copy mode) | tmux `WheelDownPane` (usually no-op) | Scroll local VT scrollback down |
| Wheel up (in copy mode) | `send-keys -X scroll-up` | `send-keys -X scroll-up` (unchanged) |
| Wheel down (in copy mode) | `send-keys -X scroll-down` | `send-keys -X scroll-down` (unchanged) |
| New `%output` arrives while scrolled back | n/a | Stay at current position (no auto-snap) |
| Key press while scrolled back | n/a | Snap to live tail, then forward key |
| Wheel scrolls to bottom | n/a | Remove `PaneScrolledBack` marker |

## Architecture

### New Component: `PaneScrolledBack`

Location: `src/ui/copy_mode.rs` (alongside `CopyModeState`)

```rust
/// Marker: this pane's local VT viewport is scrolled above the live tail.
#[derive(Component)]
pub(crate) struct PaneScrolledBack;
```

Present on a pane entity when `display_offset > 0` in the local `TerminalHandle`.
Removed when the viewport reaches the live tail (display_offset == 0).

### New `TerminalHandle` Methods

Location: `crates/ozma_tty_engine/src/handle.rs`

```rust
/// Scrolls the viewport by `delta` lines without requiring a Coalescer.
/// For display-only tmux panes: caller must call flush_emit after this.
pub fn scroll_vt_only(&mut self, delta: i32) {
    self.term.scroll_display(Scroll::Delta(delta));
}

/// Snaps the viewport to the live tail without requiring a Coalescer.
/// Returns true if the viewport was not already at the bottom.
pub fn snap_to_bottom_vt_only(&mut self) -> bool {
    if self.is_at_bottom() { return false; }
    self.term.scroll_display(Scroll::Bottom);
    true
}
```

These are coalescer-free equivalents of `scroll()` and `scroll_to_bottom()`, used
by the tmux wheel handler where no `Coalescer` component is present on pane entities.

### `forward_wheel_to_tmux` Changes

Location: `src/tmux/input.rs`

**Removed params:** `bindings: Res<KeyBindings>`  
**Kept param:** `copy_modes: Query<(), With<CopyModeState>>` (guards against the copy-mode renderer conflict)  
**Added param:** `mut handles: Query<&mut TerminalHandle>`

When `CopyModeState` is present, the copy-mode capture renderer owns the grid;
calling `flush_emit` on the live handle would fight it. VT scrollback therefore
applies only when NOT in copy mode. The per-notch loop is also collapsed — with
no per-notch branching, a single `scroll_vt_only(total_delta)` + one `flush_emit`
is equivalent:

```rust
if copy_modes.contains(entity) {
    let key = if up { "scroll-up" } else { "scroll-down" };
    mux.plan(scroll_command(pane_id, key, lines));
    return;
}
let Ok(mut handle) = handles.get_mut(entity) else { return; };
let total_delta = if up { lines as i32 } else { -(lines as i32) };
handle.scroll_vt_only(total_delta);
handle.flush_emit(&mut commands, entity);
if handle.is_at_bottom() {
    commands.entity(entity).remove::<PaneScrolledBack>();
} else {
    commands.entity(entity).insert(PaneScrolledBack);
}
```

**Deleted (dead code after change):**
- `wheel_key_name()` function (only used for the `WheelUpPane`/`WheelDownPane` binding dispatch, now removed)

**Not deleted:**
- `scroll_command()` function (still used for the copy-mode tmux scroll path above)

### `forward_keys_to_tmux` Changes

Location: `src/tmux/input.rs`

**Added param:** `mut handles: Query<&mut TerminalHandle>`

Before forwarding any key to tmux, snap to the live tail if the pane is scrolled back:

```rust
if let Some(entity) = active_entity
    && let Ok(mut handle) = handles.get_mut(entity)
    && handle.snap_to_bottom_vt_only()
{
    handle.flush_emit(&mut commands, entity);
    commands.entity(entity).remove::<PaneScrolledBack>();
}
```

### IME Commit Path

Location: `src/input/ime.rs`

The IME commit path (`read_ime_events`) sends text via `send_bytes_command`,
bypassing `forward_keys_to_tmux`. Add the same snap-to-bottom logic before
the commit is forwarded:

```rust
if let Ok(mut handle) = handles.get_mut(active_entity)
    && handle.snap_to_bottom_vt_only()
{
    handle.flush_emit(&mut commands, active_entity);
    commands.entity(active_entity).remove::<PaneScrolledBack>();
}
```

Also add `mut handles: Query<&mut TerminalHandle>` to `read_ime_events`.

### Scrollback Indicator Changes

Location: `src/ui/copy_mode_indicator.rs`

The `[offset/total]` chip is extended to show when `PaneScrolledBack` is present,
in addition to the existing `CopyModeState` trigger.

**`refresh_indicator` query:** `Or<(With<CopyModeState>, With<PaneScrolledBack>)>`

**Run condition:**
```rust
.run_if(any_with_component::<CopyModeState>.or(any_with_component::<PaneScrolledBack>))
```

**Hide on exit:** Both `On<Remove, CopyModeState>` and `On<Remove, PaneScrolledBack>`
observers check that the OTHER marker is also absent before hiding the chip.

## What Does NOT Change

- `route_tmux_output` — no auto-scroll to bottom on new output (intentional: B option)
- `CopyModePlugin` — capture-based rendering for tmux copy mode is unchanged
- Copy mode entry via keyboard shortcuts or drag-to-select — unchanged
- `CopyModeState` semantics — unchanged; `PaneScrolledBack` is orthogonal

## Files Changed

| File | Change |
|---|---|
| `crates/ozma_tty_engine/src/handle.rs` | Add `scroll_vt_only`, `snap_to_bottom_vt_only` |
| `src/ui/copy_mode.rs` | Add `PaneScrolledBack` component |
| `src/tmux/input.rs` | Rework `forward_wheel_to_tmux`; update `forward_keys_to_tmux`; delete `wheel_key_name` |
| `src/input/ime.rs` | Add snap-to-bottom before `send_bytes_command` in `read_ime_events` |
| `src/ui/copy_mode_indicator.rs` | Extend to show for `PaneScrolledBack`; add hide observer |
