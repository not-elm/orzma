# Task 3: SDK Session — Compositing Buffer

## Status: DONE

## Summary

Extended `sdk/ratatui-ozma/src/session.rs` so the reader thread buffers incoming `{"op":"compositing","handle":"…","active":bool}` lines from the ozmux host and surfaces them to widget code each frame via `FramePlacements::take_compositing`.

## Changes

### `sdk/ratatui-ozma/src/session.rs`

1. **`FramePlacements`** — added private `pending_compositing: HashMap<String, bool>` field (derives `Default` via the `#[derive(Default)]` already on the struct).

2. **`FramePlacements::take_compositing`** (`pub(crate)`) — removes and returns the buffered compositing state for a given handle. Used by Task 4's `widget.rs`.

3. **`FramePlacements::pending_compositing_for_test`** (`#[cfg(test)]`, `pub(crate)`) — read-only accessor for the pending map, following the pattern of `placements_for_test` / `focused_for_test`.

4. **`PendingCompositing` type alias** — `Arc<Mutex<HashMap<String, bool>>>`, placed alongside the existing `HandlerRegistry` and `PendingRegisters` aliases.

5. **`Ozma`** — added `pending_compositing: PendingCompositing` field.

6. **`Ozma::connect()`** — creates `pending_compositing`, passes it as a 5th argument to `spawn_reader`, and stores it in `Self`.

7. **`Ozma::frame()`** — drains the shared map into `frame.pending_compositing` via `std::mem::take`, so each call to `frame()` delivers exactly the compositing notifications that arrived since the last frame (and clears the shared buffer atomically).

8. **`spawn_reader`** — added `pending_compositing: PendingCompositing` parameter. The existing `parsed`/`op` logic was refactored from nested `is_call`/`else-if` to a cleaner `op ==` chain:
   - `op == "call"` → existing dispatch path (unchanged)
   - `op == "compositing"` → parse `handle` + `active` from the already-parsed `Value` and insert into `pending_compositing`
   - else → existing register-reply path (unchanged, NOTE comment preserved verbatim)

## Test Results

```
running 23 tests
test session::tests::pane_identity_falls_back_to_tmux_pane ... ok
test session::tests::flush_skips_degenerate_area ... ok
test session::tests::frame_drains_pending_compositing_each_call ... ok
test session::tests::pane_identity_none_when_neither_set ... ok
test session::tests::pane_identity_prefers_ozmux_token ... ok
test session::tests::flush_focus_emits_blur_on_none ... ok
test session::tests::flush_focus_emits_on_change_and_skips_unchanged ... ok
test session::tests::pane_identity_treats_empty_token_as_absent ... ok
test session::tests::flush_unmounts_vanished_handle ... ok
test session::tests::flush_emits_mount_then_skips_unchanged ... ok
test session::tests::parse_show_environment_finds_key_among_many_lines ... ok
test session::tests::parse_show_environment_keeps_equals_in_value ... ok
test session::tests::parse_show_environment_none_for_unset_marker ... ok
test session::tests::parse_show_environment_none_when_key_absent ... ok
test session::tests::parse_show_environment_reads_value ... ok
test session::tests::socket_from_tmux_handles_path_without_commas ... ok
test session::tests::socket_from_tmux_none_for_empty ... ok
test session::tests::socket_from_tmux_none_for_leading_comma ... ok
test session::tests::socket_from_tmux_takes_first_comma_field ... ok
test session::tests::take_compositing_returns_and_removes_entry ... ok
test session::tests::take_compositing_returns_none_when_absent ... ok
test session::tests::reader_thread_inserts_compositing_into_shared_map ... ok
test session::tests::reader_thread_updates_compositing_to_false ... ok

test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.06s
```

4 new tests added (5 including `frame_drains_pending_compositing_each_call`):
- `take_compositing_returns_and_removes_entry`
- `take_compositing_returns_none_when_absent`
- `frame_drains_pending_compositing_each_call`
- `reader_thread_inserts_compositing_into_shared_map`
- `reader_thread_updates_compositing_to_false`

## Build Status

`cargo build` — exits 0. Two pre-existing warnings only:
- `POWERLINE_LEFT` unused constant (from Task 1)
- `take_compositing` dead_code (expected; will be consumed in Task 4)

## Coding Rules Compliance

- `pending_compositing` field on `FramePlacements` is private (only accessed in same module via `Ozma::frame()`)
- `take_compositing` is `pub(crate)` with `///` doc comment
- `pending_compositing_for_test` is `#[cfg(test)]` only
- `PendingCompositing` type alias placed alongside existing `HandlerRegistry` / `PendingRegisters`
- `spawn_reader` mutable params order: all `PendingCompositing` / `PendingRegisters` are `Arc<Mutex<…>>` passed by value — ordering maintained
- No `// NOTE:` added (none warranted); existing NOTE comments preserved exactly
- Single contiguous import block, no new imports needed (`HashMap` already imported)
- No `mod.rs`, no narrative comments, no block comments
