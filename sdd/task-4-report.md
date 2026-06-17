# Task 4 Report: SDK widget — on_compositing_change callback

## Summary

Added `on_compositing_change` callback support to `WebviewWidget` in
`sdk/ratatui-ozma/src/widget.rs`.

## Changes

- Added `on_compositing_change: Option<Box<dyn Fn(bool) + 'a>>` field to
  `WebviewWidget<'a, W>` struct (private, no doc required).
- `WebviewWidget::new()` initializes the field to `None`.
- Added `pub fn on_compositing_change(mut self, f: impl Fn(bool) + 'a) -> Self`
  builder method on `impl<'a, W> WebviewWidget<'a, W>` with `///` doc comment.
- Updated `fallback()` builder to carry `on_compositing_change: self.on_compositing_change`
  into the returned `WebviewWidget<'a, W2>`.
- Updated `render()` to call the callback after `state.record(...)`:
  ```rust
  if let Some(active) = state.take_compositing(self.handle) {
      if let Some(cb) = &self.on_compositing_change {
          cb(active);
      }
  }
  ```

## TDD workflow

1. Added 5 failing tests to `widget.rs::tests`.
2. Confirmed `cargo test -p ratatui-ozma -- widget` failed (compilation errors).
3. Implemented the field, builder, `fallback()` carry-through, and `render()` logic.
4. All 10 widget tests pass.
5. `cargo build` exits 0.

## Test results

```
running 10 tests
test widget::tests::focused_widget_constructs ... ok
test widget::tests::fallback_is_painted ... ok
test widget::tests::on_compositing_change_not_fired_when_absent ... ok
test widget::tests::on_compositing_change_fires_when_pending ... ok
test widget::tests::on_compositing_change_fires_false ... ok
test widget::tests::focused_render_records_focused_handle ... ok
test widget::tests::on_compositing_change_consumed_from_state ... ok
test widget::tests::on_compositing_change_survives_fallback_builder ... ok
test widget::tests::unfocused_render_records_no_focus ... ok
test widget::tests::records_placement_and_blanks_cells ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 60 filtered out
```
