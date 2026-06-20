# Session picker — mouse support

Design spec — 2026-06-20
Builds on: `docs/superpowers/specs/2026-06-15-session-chooser-redesign-design.md`
(the choose-tree-style picker; this adds mouse interaction to it.)

## Context

The startup tmux session picker (`src/picker.rs`) is keyboard-only.
`handle_picker_input` reads `KeyboardInput` and drives a `selected` index:
`↑↓`/`jk` move it, `Enter` opens the selected row (attach / switch / new
session), `Esc` cancels. Rows are plain `Text` nodes with no `Interaction`.
The row list also has no height cap, so with many sessions/windows it overflows
the window.

The user wants the picker to be usable with the mouse. The repo already has a
clean mouse idiom: window-bar entries are spawned as `Button` (which carries
`Interaction`) and handled with `Changed<Interaction>` + a pointer
`CursorIcon` (`src/tmux/window_bar.rs`, `src/tmux/window_bar_input.rs`). The
wheel idiom is `MessageReader<MouseWheel>` with `MouseScrollUnit` Line/Pixel
handling (`src/tmux/input.rs`). This work reuses both.

This is an input + small-layout change to one screen. All changes are in
`src/picker.rs`.

## Settled decisions (from brainstorming)

1. **Three mouse behaviors, no click-outside-dismiss.** In scope: click a row
   to open it, hover a row to move the highlight, and wheel-scroll a long list.
   Clicking the backdrop outside the panel does **not** dismiss the picker.
2. **Single-click opens.** A single click on a row is the mouse equivalent of
   pressing `Enter` on it — it attaches/switches/creates immediately. Hover
   already provides the "preview" highlight, so no second click or
   double-click is required.
3. **Scroll area sized as a fraction of window height.** The panel is capped at
   ~65% of the window height; the row list scrolls inside that cap. Adapts to
   any window size. (Picked over a fixed visible-row count.)
4. **Activation wiring: shared helper fn (Approach A).** The keyboard-`Enter`
   path and the new click path both call one `activate_row(...)` helper rather
   than going through an `EntityEvent` + observer or a `pending_open` flag.
   Both call sites already hold the needed `&mut` params, so per
   `.claude/rules/rust.md` ("prefer a helper fn unless the apply step needs
   *isolated* `&mut`/NonSend access") a helper is the right altitude.
5. **Hover is keyboard-first on open.** Hover re-selects only when the cursor
   *enters a new row* (`Changed<Interaction>`), and hover-selection is
   suppressed on the frames just after the picker opens, so opening always
   highlights the first session rather than snapping to wherever the cursor sits.

## Design

All changes are in `src/picker.rs` (plus its `#[cfg(test)] mod tests`). The
backdrop → panel → (title + `PickerList` + footer) structure and the in-place
`sync_picker_ui` row update are preserved.

### Rows become interactive

Each row is spawned with the **`Button`** component (the `window_bar.rs` idiom —
`Button` provides the `Interaction` the UI picking backend writes), and its
linear row index is carried on the marker:

```rust
#[derive(Component)]
struct PickerRowLabel(usize); // the row's position in build_rows order
```

`With<PickerRowLabel>` queries keep working unchanged. The index is set when a
row is spawned (`PickerRowLabel(i)`); the in-place reuse branch of
`sync_picker_ui` does not touch it (row positions are stable, so the stored
index stays correct). Both the initial spawn and the despawn+respawn branch add
`Button`; the reuse branch leaves the already-present `Button` in place.

Picking is clipped to the scroll viewport, so rows scrolled out of view are not
hovered or clickable — which is the desired behavior.

### Activation helper (shared by keyboard + mouse)

The logic currently inlined in the `Enter` arm of `handle_picker_input` is
extracted to a helper that both the keyboard system and the new click system
call:

```rust
fn activate_row(
    connection: &mut TmuxConnection,
    state: &mut ConnectionState,
    next_mode: &mut NextState<AppMode>,
    configs: &OzmuxConfigsResource,
    control: Option<&ControlPlaneHandle>,
    picker: &SessionPicker,
    current_mode: &AppMode,
    row: PickerRow,
)
```

It performs the existing switch-vs-attach decision: if `connection.client()`
is `Some`, call `apply_switch`; else call `apply_attach` and, when
`should_enter_ozmux(attached, current_mode)`, set `next_mode` to
`AppMode::Ozmux`. The caller sets `picker.open = false` afterward (as the
`Enter` arm does today).

The `Enter` arm of `handle_picker_input` is rewritten to:
`picker.selected` → `rows.get(selected).copied().unwrap_or(PickerRow::NewSession)`
→ `activate_row(..)` → `picker.open = false`. Behavior is identical to today.

### Click → open (`handle_picker_click`)

New system, gated on the picker being open. Queries
`(&Interaction, &PickerRowLabel), Changed<Interaction>`. On a row whose
interaction became `Pressed`:

1. set `picker.selected = label.0`;
2. rebuild rows, take `rows.get(picker.selected).copied().unwrap_or(NewSession)`
   (the same bounds-safe path as `Enter`);
3. `activate_row(..)`;
4. `picker.open = false`.

Because step 2 mirrors the keyboard path exactly, a row whose underlying session
vanished between refresh and click is handled identically to `Enter` (no panic).

### Hover → highlight (`handle_picker_hover`)

New system, gated on the picker being open. Queries
`(&Interaction, &PickerRowLabel), Changed<Interaction>`. On a row whose
interaction became `Hovered`, set `picker.selected = label.0`.

Using `Changed<Interaction>` means selection only moves when the cursor enters a
new row; a stationary cursor never overrides keyboard navigation. A `Local<bool>`
tracking the closed→open edge suppresses hover-selection on the frames right
after open, so the initial keyboard selection (first session) stands until the
user actually moves the mouse. (This mirrors `confirm_prompt.rs`'s `armed`
`Local` pattern.)

### Pointer cursor (`picker_row_hover_cursor`)

New system, registered after `crate::input::InputPhase::Hover` (as
`window_entry_hover_cursor` is). While the picker is open: if any row's
`Interaction` is `Hovered`/`Pressed`, set the `PrimaryWindow` `CursorIcon` to
`SystemCursorIcon::Pointer`; otherwise set it to the default. The picker is a
modal backdrop, so this system is authoritative for the cursor while open rather
than relying on the hyperlink baseline system to revert. Writes are guarded by an
equality check so change detection stays honest.

### Scroll layout

- **Panel** gains `max_height: Val::Vh(65.0)`.
- **`PickerList` node** gains `overflow: Overflow::scroll_y()`,
  `flex_grow: 1.0`, and `min_height: Val::Px(0.0)`. The `min_height: 0` is the
  flexbox requirement that lets the list shrink below its content height and
  actually scroll; without it the list grows to fit all rows and the panel
  blows past its cap. Title and footer stay pinned; only the row list scrolls.

### Wheel scroll (`handle_picker_scroll`)

New system, `run_if(on_message::<MouseWheel>)` and gated on the picker being
open. Reads `MouseWheel` and adds to the `PickerList`'s `ScrollPosition.offset_y`
via a pure helper that mirrors `tmux_inline_wheel_delta`:

```rust
fn wheel_delta_px(unit: MouseScrollUnit, y: f32) -> f32 {
    match unit {
        MouseScrollUnit::Line => -y * LINE_SCROLL_PX, // one "line" ≈ a row's height
        MouseScrollUnit::Pixel => -y,
    }
}
```

(Sign chosen so wheel-down scrolls the content down, matching platform
convention; `LINE_SCROLL_PX` is a small constant ≈ one row stride.) Bevy clamps
`ScrollPosition` to the content range during layout, so over-scroll past either
end is a no-op; an empty/short list (content ≤ viewport) cannot scroll.

### Keep selection visible (`scroll_selected_into_view`)

Once the list scrolls, keyboard navigation must keep the selected row in view —
otherwise arrowing down moves the highlight off-screen. New system,
`run_if(resource_exists_and_changed::<SessionPicker>)`, scheduled after the UI
layout pass (`UiSystem::Layout`) so `ComputedNode` sizes are current for the
frame.

It reads the uniform row height from any row's `ComputedNode`, the viewport
height from the `PickerList`'s `ComputedNode`, and computes the target offset via
a pure helper:

```rust
/// New scroll offset so the row spanning [row_top, row_top+row_h] is fully
/// visible in a viewport of height viewport_h currently scrolled to `current`.
/// Scrolls up if the row is above the viewport, down if below, else unchanged.
fn reveal_offset(row_top: f32, row_h: f32, viewport_h: f32, current: f32) -> f32
```

`row_top = selected * row_stride` (rows are uniform: same font, same padding,
same `row_gap`, so `row_stride = row_h + gap`). The result is written to
`ScrollPosition.offset_y` only when it differs from the current value (guarded
write, honest change detection).

NOTE (caveat to encode): this system depends on UI layout having run this frame;
it must be ordered after `UiSystem::Layout`, and it no-ops on the frames before
any row has a computed size. A one-frame latency in the rare
open-with-huge-list case is acceptable.

### Plugin registration

`OzmuxPickerPlugin::build` adds the new systems via the existing chained-`app`
idiom. A small `fn picker_is_open(picker: Res<SessionPicker>) -> bool` run
condition gates the open-only systems (`handle_picker_click`,
`handle_picker_hover`, `handle_picker_scroll`). `picker_row_hover_cursor` is
registered after `InputPhase::Hover`; `scroll_selected_into_view` after
`UiSystem::Layout`.

## Testing

Following the file's existing style (pure helpers unit-tested; light App systems
that never touch real tmux):

- **`wheel_delta_px`** — `Line` vs `Pixel`, sign, magnitude.
- **`reveal_offset`** — row above the viewport scrolls up to it; row below
  scrolls down so its bottom is flush; row already within stays put; clamps at 0.
- **Hover App test** — using the existing `picker_input_app`-style harness, mark
  a row entity's `Interaction` as `Hovered` and run `handle_picker_hover`;
  assert `picker.selected` updates to that row's index, and that the
  post-open-suppression frame does not move it.
- Existing tests (`build_rows*`, `step_selection*`, `row_visuals*`,
  `nav_reuses_row_entities_in_place`, dispatch tests) stay green; the row-reuse
  test is extended to assert each reused row still carries `Button` +
  `PickerRowLabel(index)`.

Not unit-tested: the full click/`Enter` → attach round-trip, which shells out to
real tmux. The file already avoids exercising that path in tests; `activate_row`
is shared with the untested-by-design `Enter` arm, so click inherits the same
coverage boundary. The decision/selection and scroll math — the parts unique to
mouse support — are fully covered by the pure-fn tests above.

## Error handling & edge cases

- **Empty / short list** — `ScrollPosition` clamps to 0; wheel and
  scroll-into-view are no-ops.
- **Stale row index on click** — `rows.get(selected).unwrap_or(NewSession)`
  guards an index past the (possibly refreshed) row set, exactly as `Enter` does.
- **Cursor over a row on open** — suppressed for the post-open frames so the
  keyboard's first-session selection stands.
- **Layout not yet computed** — `scroll_selected_into_view` no-ops until rows
  have a `ComputedNode` size.

## Files touched

- `src/picker.rs` — rows spawn with `Button` + `PickerRowLabel(usize)`; panel
  `max_height` + list `overflow`/`flex_grow`/`min_height`; `activate_row`
  extracted; new systems `handle_picker_click`, `handle_picker_hover`,
  `picker_row_hover_cursor`, `handle_picker_scroll`,
  `scroll_selected_into_view`; `picker_is_open` run condition; new pure helpers
  `wheel_delta_px`, `reveal_offset`; tests extended.

No new modules; no changes outside `src/picker.rs`.

## Out of scope

- Click-outside-the-panel to dismiss.
- Double-click / click-to-highlight-then-Enter semantics.
- A draggable scrollbar thumb (wheel + keyboard only).
- Digit-key quick-jump (still a separate later follow-up, per the chooser
  redesign spec).
- Footer hint text changes (mouse is discoverable by hovering; the keyboard
  hints stay as-is).
