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
| Wheel up (in copy mode) | `send-keys -X scroll-up` | Scroll local VT scrollback up |
| Wheel down (in copy mode) | `send-keys -X scroll-down` | Scroll local VT scrollback down |
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

**Removed params:** `bindings: Res<KeyBindings>`, `copy_modes: Query<(), With<CopyModeState>>`  
**Added param:** `mut handles: Query<&mut TerminalHandle>`

The entire tmux-binding dispatch loop (`plan_forward` / `WheelUpPane` / `scroll_command`)
is replaced with direct VT scroll per notch:

```rust
let Ok(mut handle) = handles.get_mut(entity) else { return; };
let delta = if up { lines as i32 } else { -(lines as i32) };
handle.scroll_vt_only(delta);
handle.flush_emit(&mut commands, entity);
if handle.is_at_bottom() {
    commands.entity(entity).remove::<PaneScrolledBack>();
} else {
    commands.entity(entity).insert(PaneScrolledBack);
}
```

The `in_copy_mode` branch is eliminated — VT scrollback applies regardless of copy
mode state.

**Deleted (dead code after change):**
- `scroll_command()` function
- `wheel_key_name()` function

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
| `src/tmux/input.rs` | Rework `forward_wheel_to_tmux`; update `forward_keys_to_tmux`; delete `scroll_command`, `wheel_key_name` |
| `src/ui/copy_mode_indicator.rs` | Extend to show for `PaneScrolledBack`; add hide observer |
