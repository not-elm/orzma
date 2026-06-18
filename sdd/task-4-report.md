# Task 4 Report: App state machine with Mode, Cmd, ScrollAction

## Status: DONE

## Summary

Implemented `apps/ozbrowse/src/app.rs` — the pure state machine for ozbrowse.
`App::on_action` is the single entry point; it returns `Vec<Cmd>` side-effects
for `main.rs` to execute. No I/O, no SDK dependency.

## Changes

### `apps/ozbrowse/src/app.rs` (new)

1. **`ScrollAction`** (`pub(crate)`) — 8 variants: `Down`, `Up`, `HalfDown`,
   `HalfUp`, `PageDown`, `PageUp`, `Top`, `Bottom`. All carry `///` doc comments.

2. **`Cmd`** (`pub(crate)`) — 4 variants: `Scroll(ScrollAction)`, `Navigate(String)`,
   `Reload`, `Quit`. All carry `///` doc comments.

3. **`App`** (`pub(crate)`, `#[derive(Debug)]`) — fields: `mode: Mode`,
   `pending_prefix: Option<char>`, `url: String`, `address_buf: String`,
   `history: History`. All fields private.

4. **`App::new`** — takes `initial_url: String`, initializes `Mode::Normal`.

5. **Accessors** (`pub(crate)`): `mode()`, `url()`, `address_buf()`.

6. **`App::on_action`** (`pub(crate)`) — handles all 19 `Action` variants:
   - Two-key chord (`gg`) via edition-2024 let-chain on `pending_prefix.take()`
   - Address mode: `OpenAddress` clears buf and enters `Mode::Address`;
     `AddressChar`/`AddressBackspace` edit buf; `AddressConfirm` navigates
     (no-op on empty buf); `Escape` returns to `Mode::Normal`
   - History: `HistoryBack`/`HistoryForward` delegate to `History` and emit
     `Cmd::Navigate` on success, `vec![]` on empty stack
   - Mode transitions: `EnterInsert` → `Mode::Insert`; `OpenHelp` → `Mode::Help`
   - Scroll/GoBottom/Quit/Reload — direct passthrough to corresponding `Cmd`

7. **`resolve_chord`** (private) — handles `'g'` → `Cmd::Scroll(ScrollAction::Top)`.
   Private helper declared after all `pub(crate)` methods per item-ordering rule.

### `apps/ozbrowse/src/history.rs`

Added `#[derive(Debug)]` to `History` (required by `App`'s `#[derive(Debug)]`).

### `apps/ozbrowse/src/main.rs`

Added `mod app;` declaration.

## TDD workflow

1. Wrote full `app.rs` with type definitions, all method stubs (`todo!()`-free
   since implementation was clear from keymap), and 20 tests.
2. Ran `cargo test -p ozbrowse app` — failed with `E0277` (`History` missing `Debug`).
3. Added `#[derive(Debug)]` to `History`.
4. Ran `cargo test -p ozbrowse app` — 20 passed.

## Test results

```
running 20 tests
test app::tests::address_char_and_backspace_edit_buf ... ok
test app::tests::address_confirm_navigates_and_returns_to_normal ... ok
test app::tests::address_confirm_with_empty_buf_is_noop ... ok
test app::tests::dangling_prefix_then_other_key_clears_and_processes ... ok
test app::tests::enter_insert_switches_mode ... ok
test app::tests::escape_from_address_mode_returns_to_normal ... ok
test app::tests::escape_from_insert_returns_to_normal ... ok
test app::tests::gg_chord_scrolls_to_top ... ok
test app::tests::history_back_navigates_to_previous_url ... ok
test app::tests::history_back_with_empty_stack_is_noop ... ok
test app::tests::history_forward_after_back_restores_url ... ok
test app::tests::history_forward_with_empty_stack_is_noop ... ok
test app::tests::ignore_produces_no_cmds ... ok
test app::tests::new_app_starts_in_normal_mode ... ok
test app::tests::new_app_url_is_initial_url ... ok
test app::tests::open_address_enters_address_mode_with_empty_buf ... ok
test app::tests::open_help_switches_mode_to_help ... ok
test app::tests::quit_returns_quit_cmd ... ok
test app::tests::reload_returns_reload_cmd ... ok
test app::tests::scroll_actions_produce_scroll_cmds ... ok

test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 15 filtered out
```

## Commit

```
b9bc381 feat(ozbrowse): add app state machine with Mode, Cmd, ScrollAction
```

Files in commit:
- `apps/ozbrowse/src/app.rs` (new, 261 lines)
- `apps/ozbrowse/src/history.rs` (`#[derive(Debug)]` added)
- `apps/ozbrowse/src/main.rs` (`mod app;` added)
- `Cargo.lock` (ozbrowse package entry)

## Coding Rules Compliance

- `//!` module doc comment at file top
- `///` doc on all `pub(crate)` items (`ScrollAction` variants, `Cmd` variants,
  `App`, `App::new`, `App::mode`, `App::url`, `App::address_buf`, `App::on_action`)
- All items `pub(crate)`; `resolve_chord` private (declared after all pub methods)
- No narrative comments; no `// NOTE:` (none warranted)
- Mutable params before immutable throughout (self receiver exempt)
- Edition-2024 let-chain used in `on_action` for chord detection
- No `mod.rs` pattern
- Single contiguous import block at file top

---

## Fix Report (post-implementation alignment)

### Status: DONE

### Commit: `dea6372`

### Changes from spec alignment

Six deviations from the spec were corrected:

1. **`Cmd` enum** — Added `HistoryBack` and `HistoryForward` variants. Order
   adjusted to match spec: `Navigate`, `HistoryBack`, `HistoryForward`, `Reload`,
   `Scroll`, `Quit`.

2. **`on_action` HistoryBack/HistoryForward** — Removed in-place history
   manipulation (`self.history.back/forward`) and direct URL mutation. Both arms
   now simply return `vec![Cmd::HistoryBack]` / `vec![Cmd::HistoryForward]`.
   `main.rs` is responsible for calling `go_back`/`go_forward`.

3. **Four new `pub(crate)` helper methods** added to `impl App` before
   `resolve_chord`: `set_url`, `navigate`, `go_back`, `go_forward`. These give
   `main.rs` the surface it needs to drive history on behalf of `Cmd::Navigate`,
   `Cmd::HistoryBack`, `Cmd::HistoryForward`, and page-initiated URL changes.

4. **`OpenAddress`** — Changed from clearing `address_buf` to pre-filling it
   with the current URL (`self.address_buf = self.url.clone()`).

5. **`AddressConfirm`** — Changed to guard against same-URL navigation:
   returns `vec![]` when `url == self.url`. Also removed the in-place
   `self.url` and `self.history` mutation from `on_action` (delegated to
   `navigate()` method).

6. **Test suite** — Aligned 24 tests (up from 20) to the new semantics:
   - Replaced `open_address_enters_address_mode_with_empty_buf` with
     `open_address_pre_fills_current_url_and_sets_address_mode`
   - Rewrote `address_char_and_backspace_edit_buf` and
     `address_confirm_navigates_and_returns_to_normal` to account for pre-filled buf
   - Replaced old history tests that used `on_action(HistoryBack)` to assert URL
     changes (invalid under new contract) with `go_back`/`go_forward` method tests
   - Added: `history_back_forward_produce_commands`,
     `address_confirm_with_same_url_is_noop`, `navigate_updates_url_and_history`,
     `set_url_updates_url_without_touching_history`

### Test results

```
running 24 tests
test app::tests::address_char_and_backspace_edit_buf ... ok
test app::tests::address_confirm_with_same_url_is_noop ... ok
test app::tests::address_confirm_navigates_and_returns_to_normal ... ok
test app::tests::dangling_prefix_then_other_key_clears_and_processes ... ok
test app::tests::address_confirm_with_empty_buf_is_noop ... ok
test app::tests::enter_insert_switches_mode ... ok
test app::tests::escape_from_insert_returns_to_normal ... ok
test app::tests::escape_from_address_mode_returns_to_normal ... ok
test app::tests::gg_chord_scrolls_to_top ... ok
test app::tests::go_back_with_empty_stack_returns_none ... ok
test app::tests::go_back_navigates_to_previous_url ... ok
test app::tests::go_forward_after_back_restores_url ... ok
test app::tests::go_forward_with_empty_stack_returns_none ... ok
test app::tests::history_back_forward_produce_commands ... ok
test app::tests::ignore_produces_no_cmds ... ok
test app::tests::navigate_updates_url_and_history ... ok
test app::tests::new_app_starts_in_normal_mode ... ok
test app::tests::new_app_url_is_initial_url ... ok
test app::tests::open_address_pre_fills_current_url_and_sets_address_mode ... ok
test app::tests::open_help_switches_mode_to_help ... ok
test app::tests::quit_returns_quit_cmd ... ok
test app::tests::reload_returns_reload_cmd ... ok
test app::tests::scroll_actions_produce_scroll_cmds ... ok
test app::tests::set_url_updates_url_without_touching_history ... ok

test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 15 filtered out; finished in 0.00s
```

### Concerns

None.
