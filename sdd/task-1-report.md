# Task 1 Report: Add Coalescer-Free Selection Methods to TerminalHandle

## Summary

Implemented three coalescer-free selection methods (`selection_start_at_vt_only`, `selection_update_to_vt_only`, `selection_clear_vt_only`) on `TerminalHandle` following the exact pattern established by `scroll_vt_only` and `snap_to_bottom_vt_only`. These methods enable drag-select and multi-click selection in the native VT arbiter without requiring a coalescer—callers stage the selection state directly and call `flush_emit` explicitly.

## Implementation Details

### Methods Added

1. **`selection_start_at_vt_only(viewport_point, side, ty)`**
   - Coalescer-free variant of `selection_start_at` (line ~446)
   - Creates a new selection of type `ty` anchored at `viewport_point` with `side`
   - Translates viewport-relative row to alacritty `Line` using `viewport_row_to_line`
   - Immediately calls `update(anchor, opposite_side)` to avoid empty selections
   - No call to `stage_full_damage_and_arm(coalescer)` — caller must `flush_emit`
   - Doc comments explain the viewport-row translation and immediate update behavior

2. **`selection_update_to_vt_only(viewport_point, side)`**
   - Coalescer-free variant of `selection_update_to` (line ~473)
   - Extends the active selection's moving end to `viewport_point` / `side`
   - Translates viewport row using `viewport_row_to_line`
   - No-op (safe) when `Term::selection` is `None` (alacritty can wipe on alt-screen)
   - Only calls `sel.update(point, side)` on existing selection; no recomputation
   - Caller must `flush_emit` after to reach renderer

3. **`selection_clear_vt_only()`**
   - Coalescer-free variant of `selection_clear` (line ~545)
   - Drops `self.term.selection` and `self.selection_anchor`
   - No-op on coalescer — caller must `flush_emit` to broadcast the clear
   - Simplest of the three: just state mutations

### Design Alignment

All three methods:
- Follow the naming convention established by `scroll_vt_only` / `snap_to_bottom_vt_only` (public API, `_vt_only` suffix)
- Omit the `coalescer: &mut Coalescer` parameter entirely
- Do not call `stage_full_damage_and_arm(coalescer)` at all
- Rely on caller to invoke `flush_emit(commands, entity)` to reach the renderer
- Reuse existing helpers (`viewport_row_to_line`) and state fields (`selection_anchor`)
- Include doc comments explaining the coalescer-free contract

## Testing

Added **4 new tests** covering all three methods:

1. **`selection_start_at_vt_only_creates_selection_without_coalescer`**
   - Verifies that `selection_start_at_vt_only` creates a valid `Simple` selection
   - Confirms `selection_type()` and `selection_to_string()` work post-creation
   - Uses `TerminalBundle::spawn` pattern (matches existing test style)

2. **`selection_update_to_vt_only_extends_selection_without_coalescer`**
   - Chains `selection_start_at_vt_only` → `selection_update_to_vt_only`
   - Confirms selection remains active after update
   - Verifies both type and string output are valid

3. **`selection_update_to_vt_only_no_op_when_no_selection`**
   - Calls `selection_update_to_vt_only` on a handle with no active selection
   - Confirms no panic, no state change (remains `None`)
   - Mirrors the existing `selection_update_to_no_op_when_no_selection` test

4. **`selection_clear_vt_only_drops_selection_without_coalescer`**
   - Creates a selection via `selection_start_at_vt_only`
   - Confirms it becomes active (`term.selection.is_some()`)
   - Calls `selection_clear_vt_only()`
   - Confirms it becomes `None`

### Test Results

```
running 60 tests (in handle::tests and handle::accessor_tests)
............................................................
test result: ok. 60 passed; 0 failed; 0 ignored; 0 measured
```

All 4 new tests pass; all 56 existing tests continue to pass. No regressions.

## Code Location

- **Implementation**: `crates/ozma_tty_engine/src/handle.rs`, lines 551–604
  - `selection_start_at_vt_only`: lines 559–576
  - `selection_update_to_vt_only`: lines 583–596
  - `selection_clear_vt_only`: lines 601–604

- **Tests**: `crates/ozma_tty_engine/src/handle.rs`, lines 1895–1981
  - `selection_start_at_vt_only_creates_selection_without_coalescer`: lines 1897–1920
  - `selection_update_to_vt_only_extends_selection_without_coalescer`: lines 1922–1952
  - `selection_update_to_vt_only_no_op_when_no_selection`: lines 1954–1981
  - `selection_clear_vt_only_drops_selection_without_coalescer`: lines 1983–2013

## Verification

- ✅ All three methods follow the coalescer-free pattern (no `stage_full_damage_and_arm` call)
- ✅ Doc comments clearly explain the viewport-row translation and `flush_emit` contract
- ✅ Reuse existing helpers (`viewport_row_to_line`, field accessors)
- ✅ No changes to existing selection API (the coalescer-bearing variants remain unchanged)
- ✅ Safe behavior when selection is `None` (no-op in `selection_update_to_vt_only`)
- ✅ All tests pass (4 new + 56 existing = 60 total in handle module)
- ✅ Naming and style match the codebase conventions

## Next Steps

These three methods are ready for use by the drag-select arbiter (Task 1 of the drag-select feature). The arbiter can now call these methods directly and invoke `flush_emit` once per frame, eliminating the coalescer indirection for interactive selection.
