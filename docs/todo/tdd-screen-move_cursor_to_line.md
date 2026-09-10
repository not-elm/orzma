# Test cases: Screen::move_cursor_to_line

Enumerated 2026-09-10 for `VPA` (`CSI Pn d`), the Tier 1 line-addressing
half of [vt-conformance-scope.md](vt-conformance-scope.md) step 2. Sources
are `docs/references/vt510.pdf`, `xterm-ctlseqs.pdf`, and `ECMA-48.pdf`,
plus doc comments in `crates/orzma_vt/src/screen.rs`. Citations verified:
22/22 across this document and its CHA/HPA sibling,
[tdd-screen-move_cursor_to_column.md](tdd-screen-move_cursor_to_column.md).

**Revised after an independent Codex CLI review** (2026-09-10). That review
found that a wrong DECOM implementation passed every origin case in the
first pass, that the square test fixture hid a rows/columns confusion, and
that a comparison drawn against the CHA document was wrong. All corrections
are recorded in the appendix.

**The method does not exist yet**, so the Rust below does not compile against
the tree as it stands — `move_cursor_to_line` has to be added first. The
tests themselves **have** been compiled and run: every case here was
transcribed into a scratch copy of the tree against a candidate
implementation and all fourteen passed, and each case's claim about the bug
it catches was checked by injecting that bug. See "Verification" for what
that does and does not establish.

```rust
/// Addresses the cursor at a one-based line in the current column,
/// `None` for an omitted parameter.
///
/// A zero addresses the first line, the same as a one. The line is
/// resolved against the current [`OriginMode`] and clamped, so a line
/// past the addressable region stops at its edge rather than being
/// refused; the column is untouched.
///
/// # Control Functions
///
/// - `VPA` (`CSI Pn d`)
pub fn move_cursor_to_line(&mut self, line: Option<u16>)
```

`Option<u16>` mirrors [`Screen::move_cursor_to`], the sibling addressing
method, rather than `move_cursor_down(count: u16)`, a relative one. The
boundary it preserves is this: **the public addressing method normalizes the
wire value** — omitted and zero both become one, then one-based becomes
zero-based — **and `seat_cursor` resolves the origin, clamps both axes, and
cancels the deferred wrap.** `move_cursor_to` already splits the work at
exactly that seam. The method returns `()`, because the "Cursor addressing"
impl block states that none of these report damage.

## Case list

Cases are ordered High, then Medium. Every case a manual states outright is
in the High block. `TC-A` rows rest only on repository doc comments, not on
any manual.

| # | Name | Source | Priority |
| - | - | - | - |
| TC-01 | `a_one_based_line_lands_on_the_zero_based_row_of_the_same_column` | C4, C7 | High |
| TC-02 | `an_omitted_parameter_addresses_the_first_line` | C6, C10, C7 | High |
| TC-03 | `an_explicit_one_addresses_the_first_line` | C6 | High |
| TC-04 | `a_line_below_the_last_row_stops_on_the_last_line` | C5 | High |
| TC-05 | `a_margin_origin_measures_the_line_from_the_top_margin` | C4, C8, C7 | High |
| TC-06 | `a_margin_origin_measures_an_interior_line_from_the_top_margin` | C4, C8 | High |
| TC-07 | `a_line_past_the_region_clamps_to_the_bottom_margin` | C8 | High |
| TC-08 | `an_upper_left_origin_reaches_a_line_above_the_margins` | C9 | High |
| TC-09 | `an_upper_left_origin_reaches_a_line_below_the_margins` | C9 | High |
| TC-10 | `an_omitted_parameter_under_a_margin_origin_addresses_the_top_margin` | C6, C8, C10 | Medium |
| TC-11 | `the_largest_line_parameter_clamps_without_overflowing` | C8, C5 | Medium |
| TC-A1 | `a_zero_addresses_the_first_line` | ZR | Medium |
| TC-A2 | `addressing_a_line_disarms_the_deferred_wrap` | WR | Medium |
| TC-A3 | `addressing_the_line_already_held_still_disarms_the_deferred_wrap` | WR | Medium |

Source tags — **C4**: vt510 p.350 (VPA moves to vertical position Pn) ／
**C5**: vt510 p.350 (below the last line stops on the last line) ／ **C6**:
vt510 p.350 (default 1) ／ **C7**: xterm p.17 (`[row] (default = [1,column])`)
／ **C8**/**C9**: vt510 p.195 (DECOM set / reset) ／ **C10**: ECMA-48 p.26
(an empty sub-string is the default) ／ **ZR**, **WR**, **CL**, **OR**: doc
comments in `screen.rs`, quoted in full in the appendix.

Eleven of the fourteen cases are demanded by a manual. VPA is the better
specified of the two control functions this pair implements: VT510 states
its clamp outright (C5), where the column method's clamp had to come from
the **HPA** entry (C12 there) rather than from CHA. The three `TC-A` rows
come from this repository's own contracts: the zero collapse and the
deferred-wrap disarm are decisions no VT manual makes for VPA.

Add the tests to a new `mod move_cursor_to_line;` in
`crates/orzma_vt/src/screen/tests.rs`, file
`crates/orzma_vt/src/screen/tests/move_cursor_to_line.rs`. The existing
`mod move_cursor_to` covers CUP and HVP, which are different control
functions.

**Fixture choice matters on this axis.** `tall_screen()` is 4×4, so a row
index and a column index coincide and an implementation that clamps the line
against `cols` instead of `rows` passes unnoticed. Use `screen()` (4 columns
× 3 rows) wherever the assertion turns on the row bound, and reserve
`tall_screen()` for cases needing four rows to place a margin.

**These are `Screen`-level semantic tests only.** No test here shows that
`CSI Pn d` reaches this method; byte-level dispatch belongs to
`interpreter.rs` and needs its own enumeration.

Each case below carries its complete test function. The module they go in
opens with:

```rust
//! Tests for line addressing.

use super::*;
```

## TC-01 — a one-based line lands on the zero-based row of the same column

| | |
| - | - |
| Setup | `tall_screen()`; `screen.state.line = ScreenLine(0)`; `screen.state.column = GridColumn(3)` |
| Act | `screen.move_cursor_to_line(Some(3))` |
| Expect | `state.line == ScreenLine(2)` **[C4]** ／ `state.column == GridColumn(3)` **[C7]** |

The nominal case, pinning the off-by-one both ways: a one-based 3 must reach
index 2. Starting the column at 3 rather than 0 means an implementation that
carriage-returns as a side effect fails here rather than passing by
coincidence — xterm's notation `[row] (default = [1,column])` is explicit
that the column survives.

```rust
/// Asserts that a one-based line parameter lands on the zero-based row
/// while the cursor keeps the column it already occupies.
///
/// Case: an application redraws a column of a table by jumping to
/// row 3 without disturbing its horizontal position.
#[test]
fn a_one_based_line_lands_on_the_zero_based_row_of_the_same_column() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(0);
    screen.state.column = GridColumn(3);
    screen.move_cursor_to_line(Some(3));
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(3));
}
```

## TC-02 — an omitted parameter addresses the first line

| | |
| - | - |
| Setup | `tall_screen()`; `screen.state.line = ScreenLine(3)`; `screen.state.column = GridColumn(2)` |
| Act | `screen.move_cursor_to_line(None)` |
| Expect | `state.line == ScreenLine(0)` **[C6, C10]** ／ `state.column == GridColumn(2)` **[C7]** |

ECMA-48 makes an empty parameter sub-string mean the control function's
default, and VT510 gives VPA a default of 1. Seeding the row at 3 means a
method that ignores `None` entirely leaves the cursor there and fails.

```rust
/// Asserts that an omitted parameter addresses the first line,
/// leaving the column untouched.
///
/// Case: an application emits a bare `CSI d` to return to the top row
/// of the column it is drawing.
#[test]
fn an_omitted_parameter_addresses_the_first_line() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(None);
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(2));
}
```

## TC-03 — an explicit one addresses the first line

| | |
| - | - |
| Setup | `tall_screen()`; `screen.state.line = ScreenLine(3)` |
| Act | `screen.move_cursor_to_line(Some(1))` |
| Expect | `state.line == ScreenLine(0)` **[C6]** |

The other half of the default: an explicit `CSI 1 d` and an omitted
parameter have to agree. Kept separate from TC-02 because the two take
different paths through the `None | Some(0) => 1` match, and a method that
handled only the omission would still pass TC-02.

```rust
/// Asserts that an explicit one addresses the first line, agreeing
/// with the omitted-parameter default.
///
/// Case: an application that always writes its parameters out emits
/// `CSI 1 d` instead of a bare `CSI d`.
#[test]
fn an_explicit_one_addresses_the_first_line() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(3);
    screen.move_cursor_to_line(Some(1));
    assert_eq!(screen.state.line, ScreenLine(0));
}
```

## TC-04 — a line below the last row stops on the last line

| | |
| - | - |
| Setup | `screen()` — four columns by **three** rows, no scroll region, origin mode at its default |
| Act | `screen.move_cursor_to_line(Some(40))` |
| Expect | `state.line == ScreenLine(2)` **[C5]** |

The one clamp VT510 states outright for VPA: "If an attempt is made to move
the active position below the last line, then the active position stops on
the last line." The non-square fixture is the point — on `tall_screen()` the
expected row would be 3, which is also `cols - 1`, so an implementation
clamping against the wrong axis would pass. With three rows and four columns
the correct answer is 2 and the confused one is 3. The assertion also
separates clamping from the two other wrong answers: refusing the move,
which leaves the cursor where it started, and wrapping, which lands it near
the top.

```rust
/// Asserts that a line below the last row stops on the last line,
/// clamped against the row count rather than the column count.
///
/// Case: an application sized for a taller window addresses row 40
/// after the user shrinks the pane to three rows.
#[test]
fn a_line_below_the_last_row_stops_on_the_last_line() {
    let mut screen = screen();
    screen.move_cursor_to_line(Some(40));
    assert_eq!(screen.state.line, ScreenLine(2));
}
```

## TC-05 — a margin origin measures the line from the top margin

| | |
| - | - |
| Setup | `tall_screen()`; `screen.set_scroll_region(Some(2), Some(4))`; `screen.set_origin_mode(OriginMode::WithinMargins)`; then `screen.state.line = ScreenLine(3)`; `screen.state.column = GridColumn(2)` |
| Act | `screen.move_cursor_to_line(Some(1))` |
| Expect | `state.line == ScreenLine(1)` **[C4, C8]** ／ `state.column == GridColumn(2)` **[C7]** |

DECOM makes "the starting point for line numbers depend on the current top
margin setting", so under a region of rows 2..=4 the parameter 1 means the
top margin — `ScreenLine(1)` — not the top of the screen. An implementation
that treats the VPA parameter as absolute lands on `ScreenLine(0)` and
fails. The state must be assigned *after* `set_scroll_region` and
`set_origin_mode`, because both seat the cursor themselves.

```rust
/// Asserts that the line is measured from the top margin while the
/// origin is within the margins, and the column is untouched.
///
/// Case: an application with a reserved header turns on origin mode
/// and addresses the first row of its own pane.
#[test]
fn a_margin_origin_measures_the_line_from_the_top_margin() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(Some(1));
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(screen.state.column, GridColumn(2));
}
```

## TC-06 — a margin origin measures an interior line from the top margin

| | |
| - | - |
| Setup | `tall_screen()`; `screen.set_scroll_region(Some(2), Some(4))`; `screen.set_origin_mode(OriginMode::WithinMargins)`; then `screen.state.line = ScreenLine(3)`; `screen.state.column = GridColumn(2)` |
| Act | `screen.move_cursor_to_line(Some(2))` |
| Expect | `state.line == ScreenLine(2)` **[C4, C8]** ／ `state.column == GridColumn(2)` **[C7]** |

Not redundant with TC-05, and the reason is worth stating: an implementation
that treats the parameter as absolute and merely *clamps* it into the region
—`(n - 1).max(top).min(bottom)` — produces the right answer for every case
in the first pass of this document. On TC-05 it gives `0.max(1).min(3) == 1`
and on TC-07 it gives `8.max(0).min(2) == 2`, both correct by accident. Only
an interior line under a non-zero origin separates it from the real
calculation: here the correct `min(1 + 1, 3)` is 2 while the clamping
version yields 1.

```rust
/// Asserts that an interior line is measured from the top margin
/// rather than clamped into the region as an absolute row.
///
/// Case: an application with a reserved header turns on origin mode
/// and addresses the second row of its own pane.
#[test]
fn a_margin_origin_measures_an_interior_line_from_the_top_margin() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(Some(2));
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(2));
}
```

## TC-07 — a line past the region clamps to the bottom margin

| | |
| - | - |
| Setup | `tall_screen()`; `screen.set_scroll_region(Some(1), Some(3))`; `screen.set_origin_mode(OriginMode::WithinMargins)` |
| Act | `screen.move_cursor_to_line(Some(9))` |
| Expect | `state.line == ScreenLine(2)` **[C8]** |

DECOM's other half: "The cursor cannot move outside of the margins." The
barrier here is the bottom margin at `ScreenLine(2)`, not the last row at
`ScreenLine(3)`, so this case and TC-04 pin different bounds and neither
substitutes for the other. A region ending above the last row is what makes
the two distinguishable — with a region reaching row 4 both would expect
`ScreenLine(3)`.

```rust
/// Asserts that a line past the scroll region clamps to the bottom
/// margin, not to the last row, while the origin is within the margins.
///
/// Case: an application with origin mode on addresses a row below the
/// three-row pane it reserved for itself.
#[test]
fn a_line_past_the_region_clamps_to_the_bottom_margin() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(1), Some(3));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.move_cursor_to_line(Some(9));
    assert_eq!(screen.state.line, ScreenLine(2));
}
```

## TC-08 — an upper-left origin reaches a line above the margins

| | |
| - | - |
| Setup | `tall_screen()`; `screen.set_scroll_region(Some(2), Some(4))`; origin mode left at its `UpperLeftCorner` default; then `screen.state.line = ScreenLine(3)`; `screen.state.column = GridColumn(2)` |
| Act | `screen.move_cursor_to_line(Some(1))` |
| Expect | `state.line == ScreenLine(0)` **[C9]** ／ `state.column == GridColumn(2)` **[C7]** |

The mirror of TC-05, and the reason origin mode needs a case on each side:
with DECOM reset "the starting point for line numbers is independent of the
margins" and "the cursor can move outside of them". The same act that lands
on `ScreenLine(1)` in TC-05 must land on `ScreenLine(0)` here — above the
top margin — so an implementation that applies the margin origin
unconditionally fails exactly one of the two.

```rust
/// Asserts that the line is absolute and reaches above the top margin
/// while the origin is the upper-left corner.
///
/// Case: an application keeps a scrolling pane on rows 2 through 4 but
/// addresses the header row above it to update a title.
#[test]
fn an_upper_left_origin_reaches_a_line_above_the_margins() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(Some(1));
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(2));
}
```

## TC-09 — an upper-left origin reaches a line below the margins

| | |
| - | - |
| Setup | `tall_screen()`; `screen.set_scroll_region(Some(2), Some(3))`; origin mode left at its `UpperLeftCorner` default; then `screen.state.line = ScreenLine(1)`; `screen.state.column = GridColumn(2)` |
| Act | `screen.move_cursor_to_line(Some(4))` |
| Expect | `state.line == ScreenLine(3)` **[C9]** ／ `state.column == GridColumn(2)` **[C7]** |

TC-08's mirror at the other edge, and not covered by it: TC-08 catches an
implementation that raises the row to the top margin, while only this case
catches one that clamps to the bottom margin regardless of origin mode. With
a region of rows 2..=3 the bottom margin is `ScreenLine(2)`, so such an
implementation stops at row 2 where the correct answer — bounded only by the
last row with DECOM reset — is row 3.

```rust
/// Asserts that the line reaches below the bottom margin while the
/// origin is the upper-left corner, bounded only by the last row.
///
/// Case: an application keeps a scrolling pane on rows 2 and 3 and
/// addresses the status row below it.
#[test]
fn an_upper_left_origin_reaches_a_line_below_the_margins() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(Some(4));
    assert_eq!(screen.state.line, ScreenLine(3));
    assert_eq!(screen.state.column, GridColumn(2));
}
```

## TC-10 — an omitted parameter under a margin origin addresses the top margin

| | |
| - | - |
| Setup | `tall_screen()`; `screen.set_scroll_region(Some(2), Some(4))`; `screen.set_origin_mode(OriginMode::WithinMargins)`; then `screen.state.line = ScreenLine(3)`; `screen.state.column = GridColumn(2)` |
| Act | `screen.move_cursor_to_line(None)` |
| Expect | `state.line == ScreenLine(1)` **[C6, C8, C10]** ／ `state.column == GridColumn(2)` **[C7]** |

Pins the *order* of the two rules TC-02 and TC-05 pin separately: the
default resolves to a one-based line first, and the origin is applied to
that. A method that short-circuits an omitted parameter straight to
`ScreenLine(0)` — a plausible shortcut, and correct under the default origin
— lands above the top margin here and fails.

```rust
/// Asserts that an omitted parameter resolves to the default line
/// before the origin is applied, reaching the top margin rather than
/// the top of the screen.
///
/// Case: an application with origin mode on emits a bare `CSI d`
/// expecting the first row of its own pane.
#[test]
fn an_omitted_parameter_under_a_margin_origin_addresses_the_top_margin() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(None);
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(screen.state.column, GridColumn(2));
}
```

## TC-11 — the largest line parameter clamps without overflowing

| | |
| - | - |
| Setup | `tall_screen()`; `screen.set_scroll_region(Some(3), Some(4))`; `screen.set_origin_mode(OriginMode::WithinMargins)`; then `screen.state.line = ScreenLine(2)`; `screen.state.column = GridColumn(2)` |
| Act | `screen.move_cursor_to_line(Some(u16::MAX))` |
| Expect | `state.line == ScreenLine(3)` **[C8, C5]** ／ no panic |

`u16::MAX` is a reachable parameter, not a synthetic one: `CsiParam` decoding
saturates any oversized integer to it (`interpreter/csi.rs`, `first_value`),
so `CSI 999999 d` arrives here as `Some(65535)`. The region matters: rows
3..=4 put the top margin at `ScreenLine(2)`, and `65534.saturating_add(2)`
genuinely exceeds `u16::MAX` and saturates before the clamp to row 3. A top
margin of `ScreenLine(1)` would sum to exactly `u16::MAX` and never exercise
the saturation, so this case only earns its place with the margin one row
lower.

```rust
/// Asserts that the largest representable line parameter clamps to the
/// bottom margin without overflowing the origin addition.
///
/// Case: a program emits a wildly out-of-range `CSI 999999 d` while
/// origin mode is on, which the parameter decoder saturates to
/// `u16::MAX`.
#[test]
fn the_largest_line_parameter_clamps_without_overflowing() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(3), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(2);
    screen.move_cursor_to_line(Some(u16::MAX));
    assert_eq!(screen.state.line, ScreenLine(3));
    assert_eq!(screen.state.column, GridColumn(2));
}
```

## TC-A1 — a zero addresses the first line

| | |
| - | - |
| Setup | `tall_screen()`; `screen.state.line = ScreenLine(3)` |
| Act | `screen.move_cursor_to_line(Some(0))` |
| Expect | `state.line == ScreenLine(0)` **[ZR]** |

No manual states this for VPA, and ECMA-48 §5.4.2(e) covers an *empty*
sub-string rather than a written zero. VT510 states it for CUP ("If Pl is 0
or 1, then the cursor moves to line 1", p.115) and this repository has
already generalised it in `move_cursor_to`'s doc comment; the case pins that
VPA follows the same policy instead of underflowing on `line - 1`.

```rust
/// Asserts that a zero addresses the first line, the same as a one,
/// rather than underflowing the zero-based conversion.
///
/// Case: a program that builds its sequences from zero-based variables
/// emits `CSI 0 d`.
#[test]
fn a_zero_addresses_the_first_line() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(3);
    screen.move_cursor_to_line(Some(0));
    assert_eq!(screen.state.line, ScreenLine(0));
}
```

## TC-A2 — addressing a line disarms the deferred wrap

| | |
| - | - |
| Setup | `tall_screen()`; `screen.state.line = ScreenLine(1)`; `screen.state.column = GridColumn(3)`; `screen.state.pending_wrap = true` |
| Act | `screen.move_cursor_to_line(Some(3))` |
| Expect | `!screen.state.pending_wrap` **[WR]** ／ `state.line == ScreenLine(2)` **[C4]** |

The disarm follows xterm, whose `CursorSet` ends in `ResetWrap`, and it is
the behaviour a linefeed deliberately does *not* share. The cursor is seeded
at the last column, which is the only place `print` actually arms the flag,
so the setup matches the situation the scenario describes rather than an
impossible one. It comes free if the method routes through `seat_cursor`;
the case catches an implementation that writes `state.line` directly and
skips the funnel, which would leave the next printed character wrapping from
a stale flag.

```rust
/// Asserts that addressing a line discards a pending deferred wrap
/// rather than preserving it as a linefeed does.
///
/// Case: an application fills a row to its last column and then jumps
/// to another row instead of printing again.
#[test]
fn addressing_a_line_disarms_the_deferred_wrap() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    screen.move_cursor_to_line(Some(3));
    assert!(!screen.state.pending_wrap);
    assert_eq!(screen.state.line, ScreenLine(2));
}
```

## TC-A3 — addressing the line already held still disarms the deferred wrap

| | |
| - | - |
| Setup | `tall_screen()`; `screen.state.line = ScreenLine(2)`; `screen.state.column = GridColumn(3)`; `screen.state.pending_wrap = true` |
| Act | `screen.move_cursor_to_line(Some(3))` |
| Expect | `!screen.state.pending_wrap` **[WR]** ／ `state.line == ScreenLine(2)` **[C4]** |

TC-A2 only exercises the disarm when the row actually changes, so an
implementation carrying an "already there, nothing to do" early return
passes it while leaving the wrap armed. `seat_cursor` sets
`state.pending_wrap = false` unconditionally, and this case pins that: after
filling a row to its last column, re-addressing that same row must still
cancel the wrap, so the next character overwrites it instead of wrapping.

```rust
/// Asserts that addressing the line the cursor already occupies still
/// discards a pending deferred wrap.
///
/// Case: an application fills a row to its last column and then
/// addresses that very row again before printing.
#[test]
fn addressing_the_line_already_held_still_disarms_the_deferred_wrap() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    screen.move_cursor_to_line(Some(3));
    assert!(!screen.state.pending_wrap);
    assert_eq!(screen.state.line, ScreenLine(2));
}
```

## Verification

Every case was transcribed verbatim into
`crates/orzma_vt/src/screen/tests/move_cursor_to_line.rs` against the
candidate implementation below, compiled, and run. All fourteen passed. The
scratch changes were then reverted; the tree sits at `7ffaffd` with the full
`orzma_vt` suite green at 710 tests.

Passing is the weaker half of the claim. Each case also asserts that it
catches a particular wrong implementation, so those bugs were injected and
the failures observed:

| Injected bug | Tests that failed |
| - | - |
| VPA treats the parameter as absolute and merely clamps it into the region — `(n - 1).max(top).min(bottom)` | **TC-06 alone.** TC-05, TC-07, TC-08 and TC-10 all passed, which is the whole reason TC-06 was added |
| The line is clamped against `cols` instead of `rows` | **TC-04 alone** — and only because its fixture is 4×3; on the square `tall_screen()` nothing would have caught it |
| The method returns early when the target line equals the current one | **TC-A3 alone** — TC-A2 passed, which is exactly why TC-A3 exists |

The implementation used as the reference point:

```rust
pub fn move_cursor_to_line(&mut self, line: Option<u16>) {
    let line = match line {
        None | Some(0) => 1,
        Some(value) => value,
    };
    let column = self.state.column;
    self.seat_cursor(ScreenLine(line - 1), column);
}
```

The first two rows are the two findings the Codex review contributed, and
both reproduce: before TC-06 and TC-04's fixture change, each of those bugs
passed this document's entire first-pass suite.

## Appendix

### Contract table

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| C4 | VPA | unconditional | "Move cursor to line Pn. VPA causes the active position to be moved to the corresponding horizontal position at vertical position Pn." | vt510.pdf p.350, L9851-9856 |
| C5 | VPA | conditional | "If an attempt is made to move the active position below the last line, then the active position stops on the last line." | vt510.pdf p.350, L9854-9856 |
| C6 | VPA | numeric parameter | "The default value is 1." | vt510.pdf p.350, L9852-9853 |
| C7 | VPA | unconditional | "Line Position Absolute [row] (default = [1,column]) (VPA)." | xterm-ctlseqs.pdf p.17, L863 |
| C8 | DECOM | mode-dependent | "When DECOM is set, the home cursor position is at the upper-left corner of the screen, within the margins. The starting point for line numbers depends on the current top margin setting. The cursor cannot move outside of the margins." | vt510.pdf p.195, L6197-6200 |
| C9 | DECOM | mode-dependent | "When DECOM is reset, the home cursor position is at the upper-left corner of the screen. The starting point for line numbers is independent of the margins. The cursor can move outside of the margins." | vt510.pdf p.195, L6200-6203 |
| C10 | CSI parameters | numeric parameter | "An empty parameter sub-string represents a default value which depends on the control function." | ECMA-48.pdf p.26, L1022 |

Repository contracts (kind 2), each quoted verbatim from
`crates/orzma_vt/src/screen.rs`:

```
ZR — Screen::move_cursor_to
     "A zero addresses the first line or column, the same as a one."
     justifies: TC-A1

WR — Screen::seat_cursor
     "disarming the deferred wrap. The disarm follows xterm, whose
      `CursorSet` ends in `ResetWrap`, unlike a linefeed, which
      preserves the wrap on purpose."
     justifies: TC-A2 and TC-A3

CL — Screen::move_cursor_to
     "stops at its edge rather than being refused"
     justifies: TC-04, TC-07 and TC-11 assert a clamped position rather
     than an unchanged one

OR — Screen::seat_cursor
     "Seats the cursor at `line` — measured from the origin the current
      [`OriginMode`] defines"
     justifies: TC-05, TC-06, TC-08 and TC-09 differ only in origin mode,
     because the seating helper resolves the line against it

SC — Screen::seat_cursor
     "Every control function that addresses both axes ends here, and one
      that addresses the column alone ends in [`Self::seat_column`],
      which this delegates to. The origin, each clamp, and the wrap are
      therefore still decided in one place apiece and cannot drift."
     justifies: VPA routes through seat_cursor rather than writing
     state.line directly

ND — Screen, "Cursor addressing" impl block
     "None of these report damage. A move that only repositions the write
      cursor is carried by the per-chunk cursor diff"
     justifies: the method returns () and no DamageSpan case exists
```

### Terms tried

The method has no doc comment to mine, so levels 1-3 of the search ladder
were empty and every term came from level 4 (the control function named in
`vt-conformance-scope.md`, expanded to specification vocabulary).

| Term | Reached |
| - | - |
| `VPA` | vt510 p.350; ECMA-48 8.3.158 p.88 |
| `Vertical Line Position Absolute` | vt510 p.350 (the entry title) |
| `Line Position Absolute` | xterm p.17; ECMA-48 8.3.158 p.88 |
| `CSI Ps d` | xterm p.17 |
| `DECOM` / `Origin Mode` | vt510 p.195 |
| `parameter default value` | ECMA-48 5.4.2(e) p.26 |
| `VPA` in vt220.pdf | nothing — the VT220 has no VPA entry |

### Not in the specification

- **VPA's zero handling.** VT510 states the 0-or-1 collapse for CUP (p.115)
  but not for VPA, and ECMA-48 §5.4.2(e) governs an *empty* sub-string
  rather than a written zero. TC-A1 rests on `move_cursor_to`'s doc comment.
- **The deferred-wrap disarm.** No manual in `docs/references/` states what
  VPA does to a pending wrap; the policy follows xterm's `CursorSet` and is
  recorded in `seat_cursor`'s doc comment. TC-A2 and TC-A3 rest on that.
- **ECMA-48 8.3.158** defines VPA against a "data component" moving
  "parallel to the line progression" and states neither a clamp nor an
  origin interaction, so it adds nothing VT510 and xterm do not already
  carry.

### Specification errata

VT510 p.350's VPA entry carries copy-paste defects that a reader checking
the citations will hit:

- The summary line reads "VPA inquires as to the amount of free memory for
  programmable key operations" — boilerplate from an unrelated entry. The
  adjacent VPR entry carries the identical sentence under its own name.
- The Parameters block reads "Pn / is column number", which contradicts the
  Description block on the same page.
- Within the Description block itself, the standalone opening sentence at
  L9852 — "VPA causes the active position to be moved to the corresponding
  horizontal position" — is incomplete and misleading on its own. **Only
  L9854-9856 carries the usable vertical definition**, which is what C4 and
  C5 cite.

These are not specification conflicts in the sense that would suppress a
case: the manual disagrees with itself, not with another manual, and the
Description block's vertical sentences agree with xterm (C7) and ECMA-48.

### API revisions

`move_cursor_to_line` does not exist. The author settled its signature at
this enumeration's gate:

- **Taken:** `pub fn move_cursor_to_line(&mut self, line: Option<u16>)`,
  returning `()`.
- **Considered and declined:** `ScreenLine` in place of `Option<u16>`. The
  newtype is documented as a 0-based row of the active screen while a VPA
  parameter is a 1-based wire value in which 0 means 1, so the type's
  documented meaning does not fit the value. Pushing the normalization into
  `interpreter.rs` would take TC-02, TC-03, TC-10, TC-11, and TC-A1 out of
  this list; `seat_cursor(ScreenLine, GridColumn)` already provides the
  newtype typing one layer down, where the value genuinely is a coordinate.
- **Also declined:** new 1-based wire newtypes (`CursorLine`), as two new
  public types existing only for these two signatures.

No blocker was recorded for this method: every contract entry above became a
case. The restore/margin collision recorded in the sibling document is
specific to CHA, which must *preserve* an existing row; VPA overwrites the
row outright and never round-trips one through `seat_cursor`.

### Unsourced suggestions

- `VPR` (`CSI Pn e`) may not be a safe alias of the existing
  `move_cursor_down`. VT510 p.350 bounds VPR at the last *line* ("the active
  position stops at the last line") while `move_cursor_down`'s doc makes the
  bottom *margin* the barrier, per CUD. The two would differ **with DECOM
  reset and a scroll region set**; under DECOM set the cursor is confined to
  the margins anyway (C8) and no difference arises. This is a remark needing
  its own verification, not a case, and VPR's own VT510 entry carries the
  same errata as VPA's.

### Conflicts with docs/todo

None. `nvim-tree-stale-cells-ech.md` lines 44-45 list `hpa`/`vpa` as
unimplemented and line 67 records nvim issuing neither in practice, which
agrees with the zero frequency in `vt-conformance-scope.md`. Nothing under
`docs/todo/` proposes a competing signature for this method.

### Review history

- **2026-09-10, first pass.** Nine cases, self-verified only.
- **2026-09-10, Codex CLI review.** Findings applied: TC-06 added after the
  review showed that an absolute-then-clamp implementation passed every
  origin case in the first pass; TC-09 added to catch a bottom-margin clamp
  applied despite DECOM being reset; TC-04's fixture changed from the square
  `tall_screen()` to `screen()`, since a 4×4 grid hides an implementation
  clamping the row against the column count; TC-10 and TC-11 added for the
  default-before-origin ordering and the `saturating_add` overflow; TC-A3
  added for the unchanged-target wrap disarm; TC-A2's setup moved to the last
  column so it matches its scenario; TC-08 promoted to High so the High block
  really does hold every explicitly specified case; the claim that VPA's was
  "the one clamp a manual states outright for this pair" corrected, since
  VT510 states one under HPA too; the errata section narrowed to note that
  L9852 is itself misleading; the signature rationale corrected to describe
  the real normalize-then-seat boundary; the VPR remark narrowed to what the
  evidence supports. Declined: a content-preservation case, on the grounds
  that `seat_cursor` provably mutates only two coordinates and one flag, so
  the case would pin the absence of code nobody proposed.
- **2026-09-10, bodies written and executed.** Every case gained its full
  test body; the suite was compiled and run against a candidate
  implementation, and three bugs were injected to confirm the cases catch
  what they claim — including both findings this review contributed. See
  "Verification".

### How much to trust this

The Rust is checked: it compiles, it passes, and three of its cases were
shown to fail when the bug they target is introduced. What is **not**
machine-checked is the mapping from manual sentence to expected value — a
case can compile, pass, and still encode a misreading of VT510. Every
citation passed a subsequence check against the cited line span with a
dropped-qualifier guard, and every repository quote was matched verbatim with
`grep -F`, but that catches careless transcription rather than a misread.

The first pass's blind spot was neither a bad citation nor a bad expectation:
it was a **coverage gap** that every citation survived and every test passed
through. That is the failure mode to watch for here, so read TC-06's
reasoning before trusting the origin cases. Spot-check vt510.pdf p.350 (C4,
C5, C6, and the errata) and p.195 (both DECOM halves) before trusting the
rest.
