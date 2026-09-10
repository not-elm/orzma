# Test cases: Screen::insert_characters

ICH (`CSI Pn @`), enumerated from `docs/references/` — `vt510.pdf` for the
definition, `vt220.pdf` where VT510 is silent, `ECMA-48.pdf` consulted and
outranked on one point. Citations verified: 10/10. Phase 4: the list was
approved whole, and TC-07 was settled in favour of xterm's behaviour over
VT510's.

**Transcribed and landed on 2026-09-10.** All nine cases below now live in
`crates/orzma_vt/src/screen/tests/insert_characters.rs`, against the
implemented `Screen::insert_characters`; the two helpers they call, `seed_row`
and `row_glyphs`, live in `crates/orzma_vt/src/screen/tests.rs`. The Rust
blocks below are kept as the transcription record — the tests in the tree are
the living copy, so read them there rather than trusting this snapshot. What
this document still owns is the case list, the verified citations, and the
`P-` premises in the appendix.

This list is specification-derived and self-verified — nothing reviewed it but
the run that produced it. Spot-check a citation or two against the appendix
before trusting the rest.

## Case list

| # | Name | Source | Priority |
| - | - | - | - |
| TC-01 | `an_insert_opens_a_blank_at_the_cursor_and_drops_the_last_cell` | C2, C3, C4, C6, Damage | High |
| TC-02 | `an_inserted_blank_carries_the_pen_background_without_its_rendition` | C1, C3, P-FILL | High |
| TC-03 | `a_count_above_one_opens_that_many_blanks` | C2, C4, C7, Damage | High |
| TC-04 | `a_count_past_the_row_blanks_the_rest_of_the_row` | C2, C3, C4, C6, Damage | High |
| TC-05 | `an_insert_in_the_last_column_replaces_that_cell_alone` | C2, C3, C4, C6, Damage | High |
| TC-07 | `an_insert_outside_the_scroll_region_still_shifts_the_row` | P-MARGIN, Damage | High |
| TC-06 | `a_zero_count_inserts_nothing` | P-ZERO, Damage | Medium |
| TC-08 | `an_insert_disarms_the_deferred_wrap` | P-WRAP | Medium |
| TC-09 | `an_insert_below_a_scrolled_viewport_reports_no_damage` | C3, Damage | Medium |

Cases run High first, then Medium, so an author who stops after TC-07 still has
every case the specification states outright. IDs keep the numbers the gate
approved, which is why TC-07 sits above TC-06.

Source tags — **C1**: vt510 p.316 L9147-9148 (blank characters, normal
attribute) ／ **C2**: vt510 p.316 L9147-9148 (cursor stays) ／ **C3**: vt510
p.316 L9148-9149 (text moves right) ／ **C4**: vt510 p.316 L9149 (overflow
lost) ／ **C6**: vt510 p.316 L9134 (space characters) ／ **C7**: vt510 p.316
L9142-9144 (Pn, default 1) ／ **Damage**: the Damage decision table in
`.claude/skills/enumerate-test-cases/SKILL.md`.

**P-tags are not citations.** `P-FILL`, `P-MARGIN`, `P-WRAP` and `P-ZERO` are
premises the author settled in the session that produced this document, on
questions the manuals do not answer — except `P-MARGIN`, which contradicts a
verified statement and is recorded as a deliberate divergence in the appendix.
They are tagged apart from C-tags so a reader never mistakes one for the other.
Six of the nine cases rest on the specification alone.

Tests go in a new `mod insert_characters` —
`crates/orzma_vt/src/screen/tests/insert_characters.rs`, declared in
`crates/orzma_vt/src/screen/tests.rs`. No existing module exercises ICH. The
module needs `use crate::screen::grid::run::Style;` beside its `use super::*;`
for TC-02.

Two helpers are shared with the DCH enumeration, so they belong in
`screen/tests.rs` beside `seed` and `glyphs`, which only reach column zero:

```rust
/// Fills a row with glyphs from column zero, leaving the pen alone.
fn seed_row(screen: &mut Screen, line: ScreenLine, glyphs: &[char]) {
    for (column, glyph) in (0u16..).zip(glyphs) {
        screen.grid[line][column].c = *glyph;
    }
}

/// Every glyph of one row, left to right.
fn row_glyphs(screen: &Screen, line: ScreenLine) -> Vec<char> {
    (0..screen.grid.size().cols)
        .map(|column| screen.grid[line][column].c)
        .collect()
}
```

## TC-01 — one blank at the cursor, and the row's last cell falls off

| | |
| - | - |
| Setup | `screen()` (4×3); row 0 seeded `a b c d`; cursor at column 1 |
| Act | `screen.insert_characters(1)` |
| Expect | row 0 reads `a`, blank, `b`, `c` **[C3]** ／ the old `d` is gone **[C4]** ／ column 1 holds a space **[C6]** ／ the cursor stays at column 1 **[C2]** ／ returns `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

A four-column screen is the smallest one where a single insert shows all three
effects at once: something survives to the left of the cursor, something shifts
to its right, and something falls off the end. The row is seeded full so the
overflow has a value the assertion can miss if the shift copies in the wrong
direction — `copy_within` run left-to-right over an overlapping range smears the
cursor cell across the tail and still leaves column 1 blank.

```rust
/// Asserts that an insert opens one blank at the cursor, moves the
/// cells right of it one column right, and drops the cell pushed past
/// the last column, leaving the cursor where it was.
///
/// Case: a shell's line editor inserts a character into the middle of a
/// command that already fills the row.
#[test]
fn an_insert_opens_a_blank_at_the_cursor_and_drops_the_last_cell() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', blank, 'b', 'c']
    );
    assert_eq!(screen.state.column, GridColumn(1));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

## TC-02 — the blank carries the pen background but none of its rendition

| | |
| - | - |
| Setup | `screen()`; pen set to `Style::BOLD` and bg `Indexed(1)`, then `a b c d` printed with it; pen bg changed to `Indexed(4)` with `BOLD` still set; cursor at column 1 |
| Act | `screen.insert_characters(1)` |
| Expect | column 1 holds a space with `Style::empty()` **[C1]** ／ column 1 carries bg `Indexed(4)` **[P-FILL]** ／ the shifted cells keep `BOLD` and `Indexed(1)` **[C3]** |

This case splits the fill question along the line the manuals actually draw.
VT510 Table 5-16 and VT220 §4.9.1 enumerate bold, underline, blinking, negative
and invisible — and no color at all — so "the normal character attribute" reaches
the style bits and stops there, and the background is orzma's own policy
(`Pen::erase_cell`, `crates/orzma_vt/src/screen/cell.rs:86`). Printing the row
with one pen and inserting with another is what keeps the two apart: an
implementation that reuses the shifted cell's attributes, or one that stamps the
full pen including `BOLD`, passes a test that changes only one of them.

```rust
/// Asserts that the blank an insert opens carries the pen background
/// with none of the pen's rendition, while the cells it shifts keep the
/// attributes they were printed with.
///
/// Case: a TUI drawing on a colored background inserts a character into
/// text it had already drawn bold.
#[test]
fn an_inserted_blank_carries_the_pen_background_without_its_rendition() {
    let mut screen = screen();
    screen.pen_mut().style = Style::BOLD;
    screen.pen_mut().bg = Color::Indexed(1);
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c);
    }
    screen.pen_mut().bg = Color::Indexed(4);
    screen.state.column = GridColumn(1);
    screen.insert_characters(1);
    assert_eq!(screen.grid[ScreenLine(0)][1].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][1].style, Style::empty());
    assert_eq!(screen.grid[ScreenLine(0)][1].bg, Color::Indexed(4));
    assert_eq!(screen.grid[ScreenLine(0)][2].c, 'b');
    assert_eq!(screen.grid[ScreenLine(0)][2].style, Style::BOLD);
    assert_eq!(screen.grid[ScreenLine(0)][2].bg, Color::Indexed(1));
}
```

## TC-03 — a count above one opens that many blanks

| | |
| - | - |
| Setup | `screen()`; row 0 seeded `a b c d`; cursor at column 0 |
| Act | `screen.insert_characters(2)` |
| Expect | row 0 reads blank, blank, `a`, `b` **[C7]** ／ `c` and `d` are gone **[C4]** ／ the cursor stays at column 0 **[C2]** ／ returns `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

Counts above one are not hypothetical: the capture behind
`docs/todo/nvim-tree-stale-cells-ech.md` records `^[[5@` from a real Neovim
session. Two blanks at column 0 catch the implementation that loops a
single-column shift but recomputes its source from the already-shifted row,
which produces `a a b` instead.

```rust
/// Asserts that a count above one opens that many blanks in a single
/// call.
///
/// Case: an editor makes room for a two-column indent guide at the
/// start of a row it is repainting.
#[test]
fn a_count_above_one_opens_that_many_blanks() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(0);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_characters(2);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec![blank, blank, 'a', 'b']
    );
    assert_eq!(screen.state.column, GridColumn(0));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

## TC-04 — a count past the row's remaining columns blanks the rest

| | |
| - | - |
| Setup | `screen()`; row 0 seeded `a b c d`; cursor at column 2 |
| Act | `screen.insert_characters(9)` |
| Expect | columns 2 and 3 are spaces **[C6]** ／ `c` and `d` are gone **[C4]** ／ `a` and `b` are untouched **[C3]** ／ the cursor stays at column 2 **[C2]** ／ returns `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

Nine is chosen to overshoot a four-column row by more than the row is wide, so
an unclamped implementation cannot land back inside the slice by accident. This
is the case that catches an arithmetic clamp written as `count.min(cols)` rather
than `count.min(cols - column)`, and it is the one that panics rather than
failing an assertion when the shift is unclamped.

```rust
/// Asserts that a count past the columns left in the row blanks every
/// cell from the cursor to the right edge rather than shifting past it.
///
/// Case: an application asks to insert more columns than the row has
/// left while the cursor sits near the right edge.
#[test]
fn a_count_past_the_row_blanks_the_rest_of_the_row() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(2);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_characters(9);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', 'b', blank, blank]
    );
    assert_eq!(screen.state.column, GridColumn(2));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

## TC-05 — an insert in the last column replaces that cell alone

| | |
| - | - |
| Setup | `screen()`; row 0 seeded `a b c d`; cursor at column 3, the last one |
| Act | `screen.insert_characters(1)` |
| Expect | column 3 holds a space **[C6]** ／ `d` is gone **[C4]** ／ `a b c` are untouched **[C3]** ／ the cursor stays at column 3 **[C2]** ／ returns `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

The boundary where "text between the cursor and the right margin moves to the
right" has nothing left to move. A shift written as a copy of `cols - column - 1`
cells is correct here only if the zero-length case is handled; one written as an
unchecked subtraction underflows into a huge count.

```rust
/// Asserts that an insert in the last column replaces that one cell and
/// leaves every column before it untouched.
///
/// Case: the cursor rests on the final column of a full row when the
/// application inserts a character there.
#[test]
fn an_insert_in_the_last_column_replaces_that_cell_alone() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(3);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', 'b', 'c', blank]
    );
    assert_eq!(screen.state.column, GridColumn(3));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

## TC-07 — an insert outside the scroll region still shifts the row

| | |
| - | - |
| Setup | `tall_screen()` (4×4); margins `top: ScreenLine(1)`, `bottom: ScreenLine(2)`; row 3 seeded `a b c d`; cursor on line 3, below the bottom margin, at column 1 |
| Act | `screen.insert_characters(1)` |
| Expect | row 3 reads `a`, blank, `b`, `c` **[P-MARGIN]** ／ returns `Some(DamageSpan::rows(ViewportLine(3), ViewportLine(3)))` **[Damage]** |

This is the run's one deliberate divergence, and the appendix records it in full.
VT510 says ICH has no effect outside the scrolling margins (C5, verified);
xterm's `InsertChar` has no `top_marg` / `bot_marg` test at all and gates only on
the column margins, and the author settled on xterm's behaviour. The setup puts
the cursor below the bottom margin rather than above the top one because that is
the shape real applications produce — a status row pinned under a scrolling
pane. The neighbouring `insert_lines` and `delete_lines` *do* carry the vertical
guard, so an implementation copied from them will fail exactly this case, which
is why it is here rather than left implicit.

```rust
/// Asserts that an insert applies on the cursor's row even when that row
/// lies outside the scroll region, following xterm rather than VT510's
/// "no effect outside the scrolling margins".
///
/// Case: a full-screen application parks its cursor on a status row
/// below the region it scrolls and edits that row in place.
#[test]
fn an_insert_outside_the_scroll_region_still_shifts_the_row() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(2),
    });
    seed_row(&mut screen, ScreenLine(3), &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(3)),
        vec!['a', blank, 'b', 'c']
    );
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(3), ViewportLine(3)))
    );
}
```

## TC-06 — a zero count inserts nothing

| | |
| - | - |
| Setup | `screen()`; row 0 seeded `a b c d`; cursor at column 1 |
| Act | `screen.insert_characters(0)` |
| Expect | row 0 is unchanged **[P-ZERO]** ／ returns `None` **[Damage]** |

VT220 gives a zero *parameter* the meaning "insert one" (C8), and
`repeat_count` (`crates/orzma_vt/src/interpreter.rs:676`) already applies that
before `Screen` is reached, so a zero arriving here comes from inside the crate
and means what it says. Pinning it keeps a later refactor from moving the
default down a layer and silently inserting a blank on every zero.

```rust
/// Asserts that a zero count inserts nothing and reports no damage.
///
/// Case: a caller inside the crate passes a count it computed as zero.
#[test]
fn a_zero_count_inserts_nothing() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    let damage = screen.insert_characters(0);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'd']);
    assert_eq!(damage, None);
}
```

## TC-08 — an insert disarms the deferred wrap

| | |
| - | - |
| Setup | `screen()`; `a b c d` printed, which leaves the cursor on column 3 with the deferred wrap armed |
| Act | `screen.insert_characters(1)`, then `screen.print('x')` |
| Expect | `state.pending_wrap` is false after the insert **[P-WRAP]** ／ the `x` lands on row 0, column 3, rather than wrapping to row 1 **[P-WRAP]** |

No manual mentions the deferred wrap — it is xterm's `ResetWrap`, which
`InsertChar` calls — so this case rests entirely on the author's premise. The
second assertion is the one that matters: a flag check alone would pass against
an implementation that clears the flag and then leaves `print` to wrap anyway,
and `erase_in_line` in this same file takes the opposite position on the armed
wrap (`crates/orzma_vt/src/screen.rs:648`), which is what makes the behaviour
worth pinning rather than assuming.

```rust
/// Asserts that an insert disarms the deferred wrap, so the next printed
/// character stays on the cursor's row.
///
/// Case: an application fills the last column of a row and then inserts
/// a character before printing again.
#[test]
fn an_insert_disarms_the_deferred_wrap() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c);
    }
    assert!(screen.state.pending_wrap);
    screen.insert_characters(1);
    assert!(!screen.state.pending_wrap);
    screen.print('x');
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'x');
}
```

## TC-09 — an insert below a scrolled viewport reports no damage

| | |
| - | - |
| Setup | `screen()`; three `line_feed()`s from the bottom row to grow history; `viewport.offset = DisplayOffset(3)`; row 0 seeded `a b c d`; cursor on line 0, column 1 |
| Act | `screen.insert_characters(1)` |
| Expect | the grid row still shifts to `a`, blank, `b`, `c` **[C3]** ／ returns `None` **[Damage]** |

The one path where content changes and nothing is reported. Three line feeds
put three rows in history, and an offset of three pushes every live row past the
bottom of a three-row viewport, so `damage_span` sees a first row at or beyond
`rows` and yields `None`. The glyph assertion is what keeps the case honest: an
implementation that returns `None` by skipping the work entirely would otherwise
pass.

```rust
/// Asserts that an insert on a row the viewport has scrolled past still
/// shifts the row while reporting no damage.
///
/// Case: the user is reading scrollback when a program edits a line on
/// the live screen below the view.
#[test]
fn an_insert_below_a_scrolled_viewport_reports_no_damage() {
    let mut screen = screen();
    for _ in 0..3 {
        screen.state.line = ScreenLine(2);
        screen.line_feed();
    }
    screen.viewport.offset = DisplayOffset(3);
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(0);
    screen.state.column = GridColumn(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.insert_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', blank, 'b', 'c']
    );
    assert_eq!(damage, None);
}
```

## Appendix

### Contract table

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| C1 | ICH | unconditional | "The ICH sequence inserts Pn blank characters with the normal character attribute" | vt510.pdf p.316, L9147-9148 |
| C2 | ICH | unconditional | "The cursor remains at the beginning of the blank characters" | vt510.pdf p.316, L9147-9148 |
| C3 | ICH | unconditional | "Text between the cursor and right margin moves to the right" | vt510.pdf p.316, L9148-9149 |
| C4 | ICH | unconditional | "Characters scrolled past the right margin are lost" | vt510.pdf p.316, L9149 |
| C5 | ICH | conditional | "ICH has no effect outside the scrolling margins" | vt510.pdf p.316, L9149-9150 |
| C6 | ICH | unconditional | "This control function inserts one or more space (SP) characters starting at the cursor position" | vt510.pdf p.316, L9134 |
| C7 | ICH | numeric parameter | "Pn is the number of characters to insert. Default: Pn = 1" | vt510.pdf p.316, L9142-9144 |
| C8 | ICH | numeric parameter | "A parameter of 0 or 1 inserts one blank character" | vt220.pdf p.35, L1725-1726 |
| C9 | ICH | unconditional | "The cursor does not move and remains at the beginning of the inserted blank characters" | vt220.pdf p.35, L1723-1725 |
| E1 | ICH | unconditional | "The active presentation position is moved to the line home position in the active line" | ECMA-48.pdf p.60, L2755-2756 |

C9 corroborates C2 from the lower-precedence manual and produces no case of its
own. E1 is recorded because it was found and outranked, not because it governs:
see "Specification conflicts" below.

C5 and C8 produce no case in the form the manual states them. C5 is the
deliberate divergence below; C8 governs the CSI parameter, which `repeat_count`
resolves before `Screen` is called, and TC-06 pins the layer below it instead.

### Terms tried

| Term | Level | Reached |
| - | - | - |
| `ICH` | 1 | vt510 L9133 and L9147-9150 (definition), vt220 L1722 (definition), ECMA-48 L2745 (definition). Rejected first: vt510 L351 (contents), L3166 (summary table, no behaviour), L6638 (cross-reference); ECMA-48 L329 (contents), L953 and L1961 (index), L1545/L1622/L1779/L1799/L4554 (cross-references) |
| `CSI Pn @` | 1 | vt510 L9138 (format block), vt220 L1722 |
| "Insert Character" | 1, 4 | the same three definitions; vt510 L13351 and L13415 rejected (keyboard sections) |
| "blank characters" | 1 | the same vt510 and vt220 entries |
| "cursor" | 1 | too broad to search alone; reached nothing the terms above had not |
| `Screen`, cell storage, write cursor | 3 | the file's `//!` header names no control function |
| `GridSize`, nonzero axes | 2 | the `Screen` doc's invariant; no control function |
| `u16` count → "Pn", "number of characters" | 4 | vt510 L9142-9144, vt220 L1725-1726 |
| `Option<DamageSpan>` | 4 | absent from all three manuals, as expected — orzma's own contract, handled by the Damage table |

Level 1 reached statements of behaviour, so levels 2-4 were collected for the
record rather than to rescue the search.

### Not specified

Four things the cases assert that no manual states. Each is a premise, tagged
apart from the citations:

- **The background of the inserted blank** (`P-FILL`). Neither DEC manual has a
  color axis for these blanks: VT510's Table 5-16 "Visual Character Attribute
  Values" and VT220 §4.9.1 enumerate bold, underline, blinking, negative and
  invisible, and no color parameter. C1 therefore reaches the style bits and
  stops; the background follows `Pen::erase_cell`
  (`crates/orzma_vt/src/screen/cell.rs:86`) and the `bce` capability the local
  `xterm-256color` entry advertises.
- **The deferred wrap** (`P-WRAP`). No manual mentions it. xterm's `InsertChar`
  calls `ResetWrap`.
- **A zero count at the `Screen` layer** (`P-ZERO`). C8 governs the CSI
  parameter only.
- **`DamageSpan`** — orzma's own contract; the Damage decision table maps the
  specification-described change onto it.

`docs/references/xterm-ctlseqs.pdf` was read for this run and settles none of
the four: its ICH entry is one line — "CSI Ps @  Insert Ps (Blank) Character(s)
(default = 1) (ICH)." (p.12, L610) — and it states nothing about attributes,
margins, or the wrap. The xterm behaviours above come from its source, which is
not in `docs/references/` and so is cited as evidence for a premise, never as a
manual citation.

### Specification conflicts

**C5 versus TC-07 — a deliberate divergence, settled by the author.**

- vt510.pdf p.316, L9149-9150: "ICH has no effect outside the scrolling
  margins."
- xterm `util.c`: `InsertChar` carries no `top_marg` / `bot_marg` test and
  returns early only on `if (!ScrnIsColInMargins(screen, screen->cur_col))`,
  a column check. With DECLRMM disabled — orzma implements neither DECLRMM nor
  DECSLRM — that check spans the whole row, so xterm applies ICH on every row.

The author settled on xterm's behaviour, and TC-07 pins it. The manual statement
is not thereby wrong: it is the DEC terminal's behaviour, and orzma is
deliberately not reproducing it. `insert_lines` and `delete_lines` in the same
file take the DEC position on the same question, so the two now differ on
purpose.

**E1 versus C2/C9 — resolved by precedence, no case affected.** ECMA-48 § 8.3.64
has ICH move the active position to the line home position; VT510 and VT220 both
say the cursor stays where it is, and they outrank ECMA-48. Worth recording
because the neighbouring `insert_lines` does home the cursor, citing ECMA-48
§ 8.3.67 — for IL that is also what VT220 states, so the two methods diverge
here for a reason rather than by oversight.

### Unsourced suggestions

Remarks only. None of these changed a case.

- `seed_row` and `row_glyphs` belong in `screen/tests.rs` rather than in the new
  module, because the DCH enumeration needs the same two.
- The level-1 doc extractor printed `NODOC` for this method: the stub's
  `#[expect(unused_variables, ...)]` attribute is multi-line, and the extractor's
  single-line `#[` skip cannot span it, so the doc block was wiped. Placing the
  attribute above the `///` block — or deleting it once the body lands — restores
  the extractor.
- `Grid` has no in-row shift primitive today; `fill_visible_row_range`
  (`crates/orzma_vt/src/screen/grid.rs:140`) is the closest thing.

### Conflicts with docs/todo

None of API shape. Two notes:

- `vt-conformance-scope.md` §5 already concludes that ICH/DCH need a new in-row
  primitive on `Grid`, which agrees with the suggestion above. Its Tier 1 table
  records ICH's observed frequency as 0 in the session it measured.
- `nvim-tree-stale-cells-ech.md:46` records a captured `^[[5@` from a real
  Neovim session — a count of five, which is the practical justification for
  TC-03 and TC-04.
