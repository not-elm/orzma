# Test cases: Screen::move_cursor_to_column

Enumerated 2026-09-10 for `CHA` (`CSI Pn G`, **Tier 1**) and `HPA`
(``CSI Pn ` ``, **Tier 2**), the column-addressing pair in
[vt-conformance-scope.md](vt-conformance-scope.md) step 2. Sources are
`docs/references/vt510.pdf`, `xterm-ctlseqs.pdf`, and `ECMA-48.pdf`, plus
doc comments in `crates/orzma_vt/src/screen.rs`. Citations verified: 22/22
across this document and its VPA sibling.

**Revised after an independent Codex CLI review** (2026-09-10). That review
found a VT510 `HPA` clamp this enumeration had missed, two coverage gaps, and
an unconditional claim the current helper cannot honour. Every correction is
recorded in the appendix; the policy that claim forced is settled under
"API revisions".

**The method does not exist yet**, so the Rust below does not compile against
the tree as it stands — `move_cursor_to_column` has to be added first. The
tests themselves **have** been compiled and run: every case here was
transcribed into a scratch copy of the tree against a candidate
implementation and all twelve passed, and each case's claim about the bug it
catches was checked by injecting that bug. See "Verification" for what that
does and does not establish.

```rust
/// Addresses the cursor at a one-based column on the current line,
/// `None` for an omitted parameter.
///
/// A zero addresses the first column, the same as a one, and a column
/// past the last stops there. The row is never touched: this seats the
/// column alone, so neither origin resolution nor a line clamp can move
/// the cursor off the row it is on.
///
/// # Control Functions
///
/// - `CHA` (`CSI Pn G`)
/// - `HPA` (``CSI Pn ` ``)
pub fn move_cursor_to_column(&mut self, column: Option<u16>)
```

`Option<u16>` mirrors [`Screen::move_cursor_to`], the sibling addressing
method, rather than `move_cursor_up(count: u16)`, a relative one. The
boundary it preserves is this: **the public addressing method normalizes the
wire value** — omitted and zero both become one, then one-based becomes
zero-based — **and a private seating helper clamps and cancels the deferred
wrap.** `move_cursor_to` already splits the work at exactly that seam. CHA
reaches `seat_column`, the column half; `seat_cursor` reaches the same helper
after resolving the origin and clamping the line, so the two paths cannot
drift. The method returns `()`, because the "Cursor addressing"
impl block states that none of these report damage.

`HPA` shares this method because orzma has a single screen model. That is a
deliberate simplification, not an identity: ECMA-48 defines CHA against the
*presentation* component (p.48) and HPA against the *data* component (p.59),
and a terminal implementing both components separately could not merge them.
xterm merges them, and so does this terminal. Note also that VT510 states the
right-edge clamp under **HPA** (C12) and not under CHA — so sharing the
method is what gives CHA a cited clamp at all.

## Case list

Cases are ordered High, then Medium. Every case a manual states outright is
in the High block. `TC-A` rows rest only on repository doc comments, not on
any manual.

| # | Name | Source | Priority |
| - | - | - | - |
| TC-01 | `a_one_based_column_lands_on_the_zero_based_cell_of_the_same_row` | C1, C3, C11 | High |
| TC-02 | `an_omitted_parameter_addresses_the_first_column` | C2, C10, C3 | High |
| TC-03 | `a_column_move_under_a_margin_origin_leaves_the_row_where_it_was` | C3, C8, OR | High |
| TC-04 | `a_column_move_above_the_top_margin_leaves_the_row_where_it_was` | C9, LM | High |
| TC-05 | `a_column_move_below_the_bottom_margin_leaves_the_row_where_it_was` | C1, C9 | High |
| TC-06 | `a_column_past_the_right_edge_clamps_to_the_last_column` | C12 | High |
| TC-07 | `the_largest_column_parameter_clamps_without_overflowing` | C12 | Medium |
| TC-A1 | `a_zero_addresses_the_first_column` | ZR | Medium |
| TC-A2 | `addressing_a_column_disarms_the_deferred_wrap` | WR | Medium |
| TC-A3 | `addressing_the_column_already_held_still_disarms_the_deferred_wrap` | WR | Medium |
| TC-A4 | `a_row_restored_above_the_top_margin_is_preserved` | CP | Medium |
| TC-A5 | `a_row_restored_below_the_bottom_margin_is_preserved` | CP | Medium |

Source tags — **C1**: vt510 p.106 (CHA moves to the n-th character position
of the active line) ／ **C2**: vt510 p.106 (default 1) ／ **C3**: xterm p.13
(`[column] (default = [row,1])`) ／ **C8**/**C9**: vt510 p.195 (DECOM set /
reset) ／ **C10**: ECMA-48 p.26 (an empty sub-string is the default) ／
**C11**: xterm p.15 (HPA, same notation as CHA) ／ **C12**: vt510 p.312 (HPA
stops at the last position on the line) ／ **OR**, **ZR**, **CL**, **WR**,
**LM**, **CP**: doc comments in `screen.rs`, quoted in full in the appendix.

Seven of the twelve cases are demanded by a manual. The five `TC-A` rows
come from this repository's own contracts: the zero collapse and the
deferred-wrap disarm are decisions no VT manual makes for CHA or HPA.

Add the tests to a new `mod move_cursor_to_column;` in
`crates/orzma_vt/src/screen/tests.rs`, file
`crates/orzma_vt/src/screen/tests/move_cursor_to_column.rs`. The existing
`mod move_cursor_to` covers CUP and HVP, which are different control
functions. Use the `wide_screen()` (20×3), `tall_screen()` (4×4), and
`screen()` (4×3) fixtures already in `tests.rs`.

**These are `Screen`-level semantic tests only.** No test here can show that
both `CSI Pn G` and ``CSI Pn ` `` reach this method — either dispatch arm
could be missing and every case below would still pass. Byte-level dispatch
for both spellings belongs to `interpreter.rs` and needs its own enumeration.

Each case below carries its complete test function. The module they go in
opens with:

```rust
//! Tests for column addressing.

use super::*;
```

## TC-01 — a one-based column lands on the zero-based cell of the same row

| | |
| - | - |
| Setup | `wide_screen()`; `screen.state.line = ScreenLine(2)`; `screen.state.column = GridColumn(0)` |
| Act | `screen.move_cursor_to_column(Some(6))` |
| Expect | `state.column == GridColumn(5)` **[C1]** ／ `state.line == ScreenLine(2)` **[C3]** |

The nominal case, and the one that pins the off-by-one both ways: a
one-based 6 must reach index 5, not 6. Twenty columns keep the target well
inside the grid so the clamp cannot mask an arithmetic slip, and starting
the cursor on row 2 rather than row 0 means an implementation that resets
the row to the top fails here rather than passing by coincidence. This case
pins the semantics `HPA` shares **[C11]**; it does not pin that `HPA`'s
bytes reach them.

```rust
/// Asserts that a one-based column parameter lands on the zero-based
/// cell of the row the cursor already occupies.
///
/// Case: a full-screen application redraws a status field by jumping
/// to column 6 of the row it is already writing.
#[test]
fn a_one_based_column_lands_on_the_zero_based_cell_of_the_same_row() {
    let mut screen = wide_screen();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(0);
    screen.move_cursor_to_column(Some(6));
    assert_eq!(screen.state.column, GridColumn(5));
    assert_eq!(screen.state.line, ScreenLine(2));
}
```

## TC-02 — an omitted parameter addresses the first column

| | |
| - | - |
| Setup | `wide_screen()`; `screen.state.line = ScreenLine(1)`; `screen.state.column = GridColumn(7)` |
| Act | `screen.move_cursor_to_column(None)` |
| Expect | `state.column == GridColumn(0)` **[C2, C10]** ／ `state.line == ScreenLine(1)` **[C3]** |

ECMA-48 makes an empty parameter sub-string mean the control function's
default, and VT510 gives CHA a default of 1 — so a bare `CSI G` is a
carriage return that leaves the row alone. Seeding the column at 7 means a
method that ignores `None` entirely leaves the cursor there and fails.

```rust
/// Asserts that an omitted parameter addresses the first column,
/// leaving the row untouched.
///
/// Case: an application emits a bare `CSI G` to return to the left
/// edge of the row it is drawing.
#[test]
fn an_omitted_parameter_addresses_the_first_column() {
    let mut screen = wide_screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(7);
    screen.move_cursor_to_column(None);
    assert_eq!(screen.state.column, GridColumn(0));
    assert_eq!(screen.state.line, ScreenLine(1));
}
```

## TC-03 — a column move under a margin origin leaves the row where it was

| | |
| - | - |
| Setup | `tall_screen()`; `screen.set_scroll_region(Some(2), Some(4))`; `screen.set_origin_mode(OriginMode::WithinMargins)`; then `screen.state.line = ScreenLine(2)`; `screen.state.column = GridColumn(0)` |
| Act | `screen.move_cursor_to_column(Some(3))` |
| Expect | `state.line == ScreenLine(2)` **[C3, C8, OR]** ／ `state.column == GridColumn(2)` **[C1]** |

The case this list exists for. `seat_cursor`'s `line` argument is measured
from the origin **[OR]**, and under `WithinMargins` it adds the top margin:
`ScreenLine(line.0.saturating_add(origin.0).min(last.0))`. So the obvious
implementation — forward the current row unchanged, `seat_cursor(self.state.line,
column)` — double-applies the margin the moment DECOM is set. With a region
of rows 2..=4 the top margin is `ScreenLine(1)`, so a cursor on
`ScreenLine(2)` is pushed to `min(2 + 1, 3) == 3` and the assertion fails.
The state must be assigned *after* `set_scroll_region` and `set_origin_mode`,
because both seat the cursor themselves. Note the bug is invisible under the
default origin, where `origin` is zero — TC-01 cannot catch it. This is not a
hypothetical: alacritty's `goto_col` passes the absolute line into `goto`,
which double-applies the top margin under origin mode, so this case pins
against a bug that ships in a real terminal.

```rust
/// Asserts that addressing a column leaves the cursor's row unchanged
/// even while origin mode makes the seating helper's line argument
/// relative to the top margin.
///
/// Case: an application reserves rows 2 through 4 as a pane, turns on
/// origin mode, and moves along a row inside that pane.
#[test]
fn a_column_move_under_a_margin_origin_leaves_the_row_where_it_was() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(0);
    screen.move_cursor_to_column(Some(3));
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(2));
}
```

## TC-04 — a column move above the top margin leaves the row where it was

| | |
| - | - |
| Setup | `tall_screen()`; `screen.set_scroll_region(Some(2), Some(3))`; origin mode left at its `UpperLeftCorner` default; then `screen.state.line = ScreenLine(0)`; `screen.state.column = GridColumn(3)` |
| Act | `screen.move_cursor_to_column(Some(2))` |
| Expect | `state.line == ScreenLine(0)` **[C9]** ／ `state.column == GridColumn(1)` **[C1]** |

With DECOM reset the cursor may legitimately sit outside the scroll region,
and a column move must not quietly pull it back inside. The cursor starts on
row 0, above a top margin of `ScreenLine(1)`, so an implementation that
raises the row to the top margin — `row.max(top)` — fails. The column
assertion is what stops a do-nothing implementation from passing: the row
expectation alone equals the starting row.

```rust
/// Asserts that addressing a column leaves a cursor sitting above the
/// top margin on its row rather than pulling it into the region.
///
/// Case: an application keeps a scrolling pane on rows 2 and 3 but
/// moves along the header row above it to update a title.
#[test]
fn a_column_move_above_the_top_margin_leaves_the_row_where_it_was() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.state.line = ScreenLine(0);
    screen.state.column = GridColumn(3);
    screen.move_cursor_to_column(Some(2));
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(1));
}
```

## TC-05 — a column move below the bottom margin leaves the row where it was

| | |
| - | - |
| Setup | `tall_screen()`; `screen.set_scroll_region(Some(2), Some(3))`; origin mode left at its `UpperLeftCorner` default; then `screen.state.line = ScreenLine(3)`; `screen.state.column = GridColumn(3)` |
| Act | `screen.move_cursor_to_column(Some(2))` |
| Expect | `state.line == ScreenLine(3)` **[C9]** ／ `state.column == GridColumn(1)` **[C1]** |

TC-04's mirror, and not redundant with it: the two guard opposite clamps.
TC-04 catches `row.max(top)`; only this case catches `row.min(bottom)`,
which with a bottom margin of `ScreenLine(2)` would drag a cursor on row 3
up to row 2. An implementation could apply the bottom clamp and still pass
every other case in this document.

```rust
/// Asserts that addressing a column leaves a cursor sitting below the
/// bottom margin on its row rather than pulling it into the region.
///
/// Case: an application keeps a scrolling pane on rows 2 and 3 and
/// moves along the status row below it.
#[test]
fn a_column_move_below_the_bottom_margin_leaves_the_row_where_it_was() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(3);
    screen.move_cursor_to_column(Some(2));
    assert_eq!(screen.state.line, ScreenLine(3));
    assert_eq!(screen.state.column, GridColumn(1));
}
```

## TC-06 — a column past the right edge clamps to the last column

| | |
| - | - |
| Setup | `screen()` — four columns wide |
| Act | `screen.move_cursor_to_column(Some(80))` |
| Expect | `state.column == GridColumn(3)` **[C12]** |

VT510 states this clamp under HPA, whose bytes reach this same method: "If
an attempt is made to move the active position past the last position on the
line, then the active position stops at the last position on the line." Its
CHA entry states no bound, which is why an earlier revision of this document
mistakenly filed the case as repository-only. A four-column fixture makes 80
unmistakably out of range. There is no left-margin counterpart to test,
because `DECSLRM` is unimplemented **[LM]**.

```rust
/// Asserts that a column past the right edge stops at the last column
/// rather than being refused.
///
/// Case: an application sized for an 80-column window addresses
/// column 80 after the user shrinks the pane to four columns.
#[test]
fn a_column_past_the_right_edge_clamps_to_the_last_column() {
    let mut screen = screen();
    screen.move_cursor_to_column(Some(80));
    assert_eq!(screen.state.column, GridColumn(3));
}
```

## TC-07 — the largest column parameter clamps without overflowing

| | |
| - | - |
| Setup | `wide_screen()`; `screen.state.line = ScreenLine(1)`; `screen.state.column = GridColumn(0)` |
| Act | `screen.move_cursor_to_column(Some(u16::MAX))` |
| Expect | `state.column == GridColumn(19)` **[C12]** ／ no panic |

`u16::MAX` is a reachable parameter, not a synthetic one: `CsiParam` decoding
saturates any oversized integer to it
(`interpreter/csi.rs`, `first_value`), so a program emitting `CSI 999999 G`
arrives here as `Some(65535)`. The case pins that the one-based conversion
does not underflow or wrap on the way to the clamp.

```rust
/// Asserts that the largest representable column parameter clamps to
/// the last column without overflowing the one-based conversion.
///
/// Case: a program emits a wildly out-of-range `CSI 999999 G`, which
/// the parameter decoder saturates to `u16::MAX`.
#[test]
fn the_largest_column_parameter_clamps_without_overflowing() {
    let mut screen = wide_screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(0);
    screen.move_cursor_to_column(Some(u16::MAX));
    assert_eq!(screen.state.column, GridColumn(19));
    assert_eq!(screen.state.line, ScreenLine(1));
}
```

## TC-A1 — a zero addresses the first column

| | |
| - | - |
| Setup | `wide_screen()`; `screen.state.column = GridColumn(7)` |
| Act | `screen.move_cursor_to_column(Some(0))` |
| Expect | `state.column == GridColumn(0)` **[ZR]** |

No manual states this for CHA or HPA, and ECMA-48 §5.4.2(e) covers omission
only, not a written zero. VT510 states it for CUP ("If Pc is 0 or 1, then
the cursor moves to column 1", p.115) and this repository has already
generalised it in `move_cursor_to`'s doc comment; the case pins that CHA
follows the same policy instead of underflowing on `column - 1`.

```rust
/// Asserts that a zero addresses the first column, the same as a one,
/// rather than underflowing the zero-based conversion.
///
/// Case: a program that builds its sequences from zero-based variables
/// emits `CSI 0 G`.
#[test]
fn a_zero_addresses_the_first_column() {
    let mut screen = wide_screen();
    screen.state.column = GridColumn(7);
    screen.move_cursor_to_column(Some(0));
    assert_eq!(screen.state.column, GridColumn(0));
}
```

## TC-A2 — addressing a column disarms the deferred wrap

| | |
| - | - |
| Setup | `wide_screen()`; `screen.state.line = ScreenLine(1)`; `screen.state.column = GridColumn(19)`; `screen.state.pending_wrap = true` |
| Act | `screen.move_cursor_to_column(Some(3))` |
| Expect | `!screen.state.pending_wrap` **[WR]** ／ `state.column == GridColumn(2)` **[C1]** |

The disarm follows xterm, whose `CursorSet` ends in `ResetWrap`, and it is
the behaviour a linefeed deliberately does *not* share. The cursor is seeded
at the last column, which is the only place `print` actually arms the flag,
so the setup matches the situation the scenario describes rather than an
impossible one. It comes free if the method routes through `seat_column`;
the case catches an implementation that writes `state.column` directly and
skips that helper, which would leave the next printed character wrapping from
a stale flag.

```rust
/// Asserts that addressing a column discards a pending deferred wrap
/// rather than preserving it as a linefeed does.
///
/// Case: an application fills a row to its last column and then jumps
/// back along that row instead of printing again.
#[test]
fn addressing_a_column_disarms_the_deferred_wrap() {
    let mut screen = wide_screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(19);
    screen.state.pending_wrap = true;
    screen.move_cursor_to_column(Some(3));
    assert!(!screen.state.pending_wrap);
    assert_eq!(screen.state.column, GridColumn(2));
}
```

## TC-A3 — addressing the column already held still disarms the deferred wrap

| | |
| - | - |
| Setup | `wide_screen()`; `screen.state.line = ScreenLine(1)`; `screen.state.column = GridColumn(19)`; `screen.state.pending_wrap = true` |
| Act | `screen.move_cursor_to_column(Some(20))` |
| Expect | `!screen.state.pending_wrap` **[WR]** ／ `state.column == GridColumn(19)` **[C1]** |

TC-A2 only exercises the disarm when the column actually changes, so an
implementation carrying an "already there, nothing to do" early return
passes it while leaving the wrap armed. `seat_cursor` sets
`state.pending_wrap = false` unconditionally, and this case pins that: after
filling the last column, re-addressing that same column must still cancel
the wrap, so the next character overwrites it instead of wrapping.

```rust
/// Asserts that addressing the column the cursor already occupies
/// still discards a pending deferred wrap.
///
/// Case: an application fills a row to its last column and then
/// addresses that very column again before printing.
#[test]
fn addressing_the_column_already_held_still_disarms_the_deferred_wrap() {
    let mut screen = wide_screen();
    screen.state.line = ScreenLine(1);
    screen.state.column = GridColumn(19);
    screen.state.pending_wrap = true;
    screen.move_cursor_to_column(Some(20));
    assert!(!screen.state.pending_wrap);
    assert_eq!(screen.state.column, GridColumn(19));
}
```

## TC-A4 — a row restored above the top margin is preserved

| | |
| - | - |
| Setup | `tall_screen()`; `set_origin_mode(OriginMode::WithinMargins)`; `save_checkpoint()`; `set_scroll_region(Some(2), Some(4))`; `restore_checkpoint()` |
| Act | `screen.move_cursor_to_column(Some(3))` |
| Expect | `state.line == ScreenLine(0)` **[CP]** ／ `state.column == GridColumn(2)` **[C1]** |

`restore_checkpoint` puts the saved row back verbatim, so this sequence of
public operations — DECOM on, DECSC, DECSTBM, DECRC — leaves the cursor above
the current top margin. Routing the line through `seat_cursor` would relocate
it, because `saturating_sub` cannot express a negative relative line. The test
asserts the intermediate state too, so it documents that the restore really
does leave the cursor out of bounds before CHA is called.

```rust
/// Asserts that a row restored above the current top margin is left
/// where it is rather than reseated onto that margin.
///
/// Case: an application saves the cursor with origin mode on, moves the
/// scroll region down, restores the cursor, and then addresses a column.
#[test]
fn a_row_restored_above_the_top_margin_is_preserved() {
    let mut screen = tall_screen();
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.save_checkpoint();
    screen.set_scroll_region(Some(2), Some(4));
    screen.restore_checkpoint();
    assert_eq!(screen.state.line, ScreenLine(0));
    screen.move_cursor_to_column(Some(3));
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(2));
}
```

## TC-A5 — a row restored below the bottom margin is preserved

| | |
| - | - |
| Setup | `tall_screen()`; `set_origin_mode(OriginMode::WithinMargins)`; `move_cursor_to(Some(4), Some(1))`; `save_checkpoint()`; `set_scroll_region(Some(1), Some(3))`; `restore_checkpoint()` |
| Act | `screen.move_cursor_to_column(Some(2))` |
| Expect | `state.line == ScreenLine(3)` **[CP]** ／ `state.column == GridColumn(1)` **[C1]** |

TC-A4's mirror, and the one that survives more fixes than it looks. TC-A4's
relocation comes from the origin subtraction, so a smarter origin round-trip
would remove it; this one comes from `seat_cursor`'s `.min(last.0)`, where
`last` is the bottom margin under DECOM, and therefore fires on *any* path
through that helper. Only seating the column alone satisfies both.

```rust
/// Asserts that a row restored below the current bottom margin is left
/// where it is rather than clamped onto that margin.
///
/// Case: an application saves the cursor on the last row with origin
/// mode on, shrinks the scroll region, restores the cursor, and then
/// addresses a column.
#[test]
fn a_row_restored_below_the_bottom_margin_is_preserved() {
    let mut screen = tall_screen();
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.move_cursor_to(Some(4), Some(1));
    screen.save_checkpoint();
    screen.set_scroll_region(Some(1), Some(3));
    screen.restore_checkpoint();
    assert_eq!(screen.state.line, ScreenLine(3));
    screen.move_cursor_to_column(Some(2));
    assert_eq!(screen.state.line, ScreenLine(3));
    assert_eq!(screen.state.column, GridColumn(1));
}
```

## Verification

Every case was transcribed verbatim into
`crates/orzma_vt/src/screen/tests/move_cursor_to_column.rs` against the
candidate implementation below, compiled, and run. All twelve passed. The
scratch changes were then reverted; the tree sits at `7ffaffd` with the full
`orzma_vt` suite green at 710 tests.

Passing is the weaker half of the claim. Each case also asserts that it
catches a particular wrong implementation, so those bugs were injected and
the failures observed:

| Injected bug | Tests that failed |
| - | - |
| CHA forwards the absolute row to `seat_cursor`, double-applying the top margin under DECOM | **TC-03**, plus TC-A4 and TC-A5 |
| CHA routes the line through `seat_cursor` with an origin round-trip (the reseat policy this document previously carried) | **TC-A4 and TC-A5 alone** — the other 24 cases passed, so the pair pins the policy and nothing else |
| The method returns early when the target column equals the current one | **TC-A3 alone** — TC-A2 passed, which is exactly why TC-A3 exists |

The implementation used as the reference point:

```rust
pub fn move_cursor_to_column(&mut self, column: Option<u16>) {
    let column = match column {
        None | Some(0) => 1,
        Some(value) => value,
    };
    self.seat_column(GridColumn(column - 1));
}

/// The column half of `seat_cursor`, extracted so CHA can reach it
/// without the line half. `seat_cursor` now delegates to it.
fn seat_column(&mut self, column: GridColumn) {
    let cols = self.grid.size().cols;
    self.state.column = GridColumn(column.0.min(cols - 1));
    self.state.pending_wrap = false;
}
```

Seating the column alone is what makes the row unconditionally preserved. An
earlier revision routed the line through `seat_cursor` with an origin
round-trip; TC-A4 and TC-A5 are the cases that reject it, and the second
mutation row above shows they reject nothing else.

## Appendix

### Contract table

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| C1 | CHA | unconditional | "The active position is moved to the n-th character position of the active line." | vt510.pdf p.106, L4117-4118 |
| C2 | CHA | numeric parameter | "Move the active position to the n-th character of the active line. Default: 1." | vt510.pdf p.106, L4105-4107 |
| C3 | CHA | unconditional | "Cursor Character Absolute [column] (default = [row,1]) (CHA)." | xterm-ctlseqs.pdf p.13, L629 |
| C8 | DECOM | mode-dependent | "When DECOM is set, the home cursor position is at the upper-left corner of the screen, within the margins. The starting point for line numbers depends on the current top margin setting. The cursor cannot move outside of the margins." | vt510.pdf p.195, L6197-6200 |
| C9 | DECOM | mode-dependent | "When DECOM is reset, the home cursor position is at the upper-left corner of the screen. The starting point for line numbers is independent of the margins. The cursor can move outside of the margins." | vt510.pdf p.195, L6200-6203 |
| C10 | CSI parameters | numeric parameter | "An empty parameter sub-string represents a default value which depends on the control function." | ECMA-48.pdf p.26, L1022 |
| C11 | HPA | unconditional | "Character Position Absolute [column] (default = [row,1]) (HPA)." | xterm-ctlseqs.pdf p.15, L783-784 |
| C12 | HPA | conditional | "HPA causes the active position to be moved to the n-th horizontal position of the active line. If an attempt is made to move the active position past the last position on the line, then the active position stops at the last position on the line." | vt510.pdf p.312, L9089-9091 |

Component distinction, cited but deriving no case, because orzma models one
screen rather than separate presentation and data components:

- ECMA-48 p.48, L2140-2141 — "CHA causes the active presentation position to
  be moved to character position n in the active line in the presentation
  component".
- ECMA-48 p.59, L2694-2695 — "HPA causes the active data position to be
  moved to character position n in the active line (the line in the data
  component that contains the active data position)".

Repository contracts (kind 2), each quoted verbatim from
`crates/orzma_vt/src/screen.rs`:

```
OR — Screen::seat_cursor
     "Seats the cursor at `line` — measured from the origin the current
      [`OriginMode`] defines"
     justifies: why CHA must not reach seat_cursor at all. The row it
     preserves is absolute, so forwarding it under WithinMargins
     double-applies the top margin (TC-03), and converting first still
     leaves the bottom-margin clamp (TC-A5).

WR — Screen::seat_cursor
     "disarming the deferred wrap. The disarm follows xterm, whose
      `CursorSet` ends in `ResetWrap`, unlike a linefeed, which
      preserves the wrap on purpose."
     justifies: TC-A2 and TC-A3

SC — Screen::seat_cursor
     "Every control function that addresses both axes ends here, and one
      that addresses the column alone ends in [`Self::seat_column`],
      which this delegates to. The origin, each clamp, and the wrap are
      therefore still decided in one place apiece and cannot drift."
     justifies: CHA reaches the column clamp and the wrap disarm through
     seat_column rather than writing state.column directly.

ND — Screen, "Cursor addressing" impl block
     "None of these report damage. A move that only repositions the write
      cursor is carried by the per-chunk cursor diff"
     justifies: the method returns () and no DamageSpan case exists

ZR — Screen::move_cursor_to
     "A zero addresses the first line or column, the same as a one."
     justifies: TC-A1

CL — Screen::move_cursor_to
     "stops at its edge rather than being refused"
     justifies: the clamp is a stop, not a refusal, in TC-06 and TC-07

LM — Screen::move_cursor_left
     "The page border is the barrier, not a margin: this terminal has
      no left margin, because `DECSLRM` needs the vertical split screen
      mode it does not implement."
     justifies: no margin affects the column axis, so TC-04 and TC-05
     assert row preservation and TC-06 the page edge

CP — Screen::restore_checkpoint
     "The saved position is put back verbatim. Restoring an origin mode
      whose margins moved in between can therefore seat the cursor
      outside them; DECSC saves no margins to clamp against, and the
      manuals leave the collision undefined."
     justifies: TC-A4 and TC-A5, and the preservation policy settled under
     "API revisions"
```

### Terms tried

The method has no doc comment to mine, so levels 1-3 of the search ladder
were empty and every term came from level 4 (the control functions named in
`vt-conformance-scope.md`, expanded to specification vocabulary).

| Term | Reached |
| - | - |
| `CHA` | vt510 p.106; ECMA-48 8.3.9 p.48 |
| `Cursor Horizontal Absolute` | vt510 p.106 (the entry title) |
| `Cursor Character Absolute` | xterm p.13; ECMA-48 8.3.9 p.48 |
| `CSI Ps G` | xterm p.13 |
| `HPA` | xterm p.15; **vt510 p.312** — missed on the first pass |
| `Character Position Absolute` | xterm p.15; ECMA-48 8.3.57 p.59 |
| `Horizontal Position Absolute` | vt510 p.312 (the entry title) |
| `DECOM` / `Origin Mode` | vt510 p.195 |
| `parameter default value` | ECMA-48 5.4.2(e) p.26 |
| `CHA` / `HPA` in vt220.pdf | nothing — the VT220 has neither entry |

The first pass searched `HPA` only in `xterm-ctlseqs.txt`, having already
found CHA in VT510, and so never reached VT510's own HPA entry and its
clamp (C12). The lesson is the skill's own rule: exhaust every term against
every manual, not against the first manual that answers.

### Not in the specification

- **CHA's zero handling.** VT510 states the 0-or-1 collapse for CUP (p.115)
  but not for CHA or HPA, and ECMA-48 §5.4.2(e) governs an *empty*
  sub-string rather than a written zero. TC-A1 rests on `move_cursor_to`'s
  doc comment.
- **The deferred-wrap disarm.** No manual in `docs/references/` says what
  CHA does to a pending wrap; the policy follows xterm's `CursorSet` and is
  recorded in `seat_cursor`'s doc comment. TC-A2 and TC-A3 rest on that.
- **The right-edge clamp is no longer listed here.** An earlier revision
  filed it as repository-only; VT510 states it under HPA (C12).
- **ECMA-48 8.3.9** states neither a clamp nor a default beyond `Pn = 1`, so
  it adds nothing VT510 and xterm do not already carry.

### API revisions

`move_cursor_to_column` does not exist. The author settled its signature at
this enumeration's gate:

- **Taken:** `pub fn move_cursor_to_column(&mut self, column: Option<u16>)`,
  returning `()`.
- **Considered and declined:** `GridColumn` in place of `Option<u16>`. The
  newtype is documented as a 0-based grid coordinate while a CHA parameter
  is a 1-based wire value in which 0 means 1, so the normalization would move
  into `interpreter.rs` — splitting parameter handling across two layers for
  CHA while CUP keeps it in `Screen`. TC-02, TC-A1, and TC-07 would
  leave this list for a separate `interpreter.rs` enumeration. The axis
  typing it buys is small here, because the value is constructed at the call
  site and consumed one line later, and `seat_cursor(ScreenLine, GridColumn)`
  still carries the newtypes one layer down.
- **Also declined:** new 1-based wire newtypes (`CursorColumn`), as two new
  public types existing only for these two signatures.

#### Settled — restored rows outside the margins

`restore_checkpoint` puts the saved row back verbatim and re-applies the saved
origin mode without re-seating **[CP]**, so DECOM-on → DECSC → DECSTBM → DECRC
leaves the cursor outside the current margins. Any path through `seat_cursor`
then moves it, in two independent ways: `saturating_sub` cannot express a
negative relative line (above the top margin), and `.min(last.0)` clamps to
the bottom margin under DECOM (below it). The second fires on every
`seat_cursor` path, so no origin-conversion fix removes it.

**Decision (2026-09-11, revised the same day): preserve the row.** CHA seats
the column alone. The column clamp and the wrap disarm live in a new private
`seat_column` helper that `seat_cursor` delegates to, so the two paths cannot
drift **[SC]**.

The first decision that day accepted the reseat, on the grounds that no manual
defines the state. A review overturned it on two counts. Codex found the
below-margin case, which the original choice had not considered — the policy
covered two exceptions, not one. And a survey of six implementations found
none that moves the row on CHA: kitty (`screen_cursor_to_column`), GNOME VTE
(`set_cursor_column`), Microsoft Terminal (`Offset::Unchanged()`) and tmux
(`py = -1`) never write the row field, while xterm preserves it by signed
cancellation. Accepting the reseat would have made orzma the only one of seven
to relocate the cursor, and would have done it twice.

DECRC's own restore behaviour is genuinely divergent (xterm re-clamps, kitty
and VTE clamp only to screen bounds, alacritty does not save DECOM at all), so
"the manuals leave the collision undefined" still stands. What the survey
settled is the consequence, not the premise.

### Unsourced suggestions

- `HPR` (`CSI Pn a`) and `VPR` (`CSI Pn e`) may not be safe aliases of the
  existing `move_cursor_right` / `move_cursor_down`. For **VPR** the concern
  is concrete: VT510 p.350 bounds it at the last *line* while
  `move_cursor_down`'s doc makes the bottom *margin* the barrier, so the two
  would differ **with DECOM reset and a scroll region set** — under DECOM set
  the cursor is confined to the margins anyway (C8) and no difference arises.
  For **HPR** no difference has been demonstrated at all; it is listed only
  because the scope doc groups the three. Both are remarks needing their own
  verification, not cases.

### Conflicts with docs/todo

None. `nvim-tree-stale-cells-ech.md` lines 44-45 list `hpa`/`vpa` as
unimplemented and line 67 records nvim issuing neither in practice, which
agrees with the zero frequency in `vt-conformance-scope.md`. Nothing under
`docs/todo/` proposes a competing signature for this method.

### Review history

- **2026-09-10, first pass.** Eight cases, self-verified only.
- **2026-09-10, Codex CLI review.** Findings applied: the missed VT510 HPA
  clamp added as C12, promoting the right-edge case from repository-only to
  manual-backed; HPA relabelled Tier 2; TC-05, TC-07, and TC-A3 added to
  close the below-bottom-margin, overflow, and unchanged-target gaps; TC-A2's
  setup moved to the right edge so it matches its scenario; TC-04 promoted to
  High so the High block really does hold every explicitly specified case;
  the signature rationale corrected to describe the real
  normalize-then-seat boundary; the HPR/VPR remark narrowed to what the
  evidence supports; the CHA/HPA equivalence qualified against ECMA-48's
  component distinction; the restore/margin collision recorded as an open
  decision. Declined: a content-preservation case, on the grounds that
  `seat_cursor` provably mutates only two coordinates and one flag, so the
  case would pin the absence of code nobody proposed.
- **2026-09-10, bodies written and executed.** Every case gained its full
  test body; the suite was compiled and run against a candidate
  implementation, and two bugs were injected to confirm the cases catch what
  they claim. See "Verification".
- **2026-09-10, the explicit-one case removed at the author's request** as
  unnecessary: an explicit `CSI 1 G` takes the same `Some(value) => value` arm
  as TC-01 and differs only in landing on that arm's boundary value, which did
  not earn a case of its own. The remaining cases were renumbered to stay
  contiguous, and every reference in this document — the entries above
  included — uses the current numbering.
- **2026-09-11, the restore/margin collision settled** in favour of accepting
  the reseat. TC-A4 added and verified.
- **2026-09-11, that decision reversed after a spec review.** Codex found a
  second reseat the first decision had missed (below the bottom margin, caused
  by `seat_cursor`'s clamp rather than by the origin subtraction), and a
  six-implementation survey found none that moves the row on CHA. The policy
  is now preservation: CHA seats the column alone through a new `seat_column`
  helper. TC-A4 was rewritten to assert preservation and TC-A5 added for the
  below-margin case; both were verified, and a mutation confirms they reject
  the old policy and nothing else.

### How much to trust this

The Rust is checked: it compiles, it passes, and two of its cases were shown
to fail when the bug they target is introduced. What is **not** machine-
checked is the mapping from manual sentence to expected value — a case can
compile, pass, and still encode a misreading of VT510. Every citation passed
a subsequence check against the cited line span with a dropped-qualifier
guard, and every repository quote was matched verbatim with `grep -F`, but
that catches careless transcription rather than a misread.

The failure mode this process actually exhibited was neither: the missed HPA
clamp was an **incomplete search**, and every one of the 19 original
citations was genuine. Two cases rest on judgement rather than on a run:
TC-06's clamp traces to HPA rather than CHA, and TC-A4/TC-A5 encode a policy
the manuals do not settle — one chosen to match six other terminals rather
than derived from a specification. Spot-check vt510.pdf p.106 and p.312 and
xterm-ctlseqs.pdf p.13 before trusting the rest.
