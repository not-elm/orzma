# Test cases: Screen::delete_characters

DCH (`CSI Pn P`), enumerated from `docs/references/` — `vt510.pdf` for the
definition, `vt220.pdf` for the per-character blank it creates, `ECMA-48.pdf`
corroborating the shift. Citations verified: 12/12. Phase 4: the list was
approved whole, including the `P-CURSOR` premise and the same deliberate
divergence from the scrolling-margin rule that the ICH run settled.

**Transcribed and landed on 2026-09-10.** All nine cases below now live in
`crates/orzma_vt/src/screen/tests/delete_characters.rs`, against the
implemented `Screen::delete_characters`; the two helpers they call, `seed_row`
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
| TC-01 | `a_delete_closes_the_gap_and_blanks_the_right_margin` | D2, D4, P-CURSOR, Damage | High |
| TC-02 | `the_shifted_cells_keep_their_attributes_and_the_new_blank_takes_the_pen_background` | D3, D4, P-FILL | High |
| TC-03 | `a_count_above_one_deletes_that_many_and_blanks_as_many_columns` | D2, D8, P-CURSOR, Damage | High |
| TC-04 | `a_count_past_the_row_deletes_only_the_remaining_characters` | D6, P-CURSOR, Damage | High |
| TC-05 | `a_delete_in_the_last_column_blanks_that_cell_alone` | D2, D4, P-CURSOR, Damage | High |
| TC-07 | `a_delete_outside_the_scroll_region_still_closes_the_gap` | P-MARGIN, Damage | High |
| TC-06 | `a_zero_count_deletes_nothing` | P-ZERO, Damage | Medium |
| TC-08 | `a_delete_disarms_the_deferred_wrap` | P-WRAP | Medium |
| TC-09 | `a_delete_below_a_scrolled_viewport_reports_no_damage` | D2, Damage | Medium |

Cases run High first, then Medium. IDs keep the numbers the gate approved, which
is why TC-07 sits above TC-06.

Source tags — **D2**: vt510 p.121 L4403-4404 (remaining characters move left) ／
**D3**: vt510 p.121 L4404 (attributes move with the characters) ／ **D4**:
vt510 p.121 L4404-4405 (blank spaces, no visual attributes, at the right
margin) ／ **D6**: vt510 p.121 L4398-4399 (a count past the row deletes only
the rest) ／ **D8**: vt220 p.35 L1731-1733 (one space per deleted character) ／
**Damage**: the Damage decision table in
`.claude/skills/enumerate-test-cases/SKILL.md`.

**P-tags are not citations.** `P-FILL`, `P-MARGIN`, `P-WRAP` and `P-ZERO` carry
the meanings the ICH run settled. `P-CURSOR` is specific to this method and is
the one premise that exists because the manuals are *silent* where they were
explicit for ICH — see "Not specified". Five of the nine cases rest on the
specification alone.

Tests go in a new `mod delete_characters` —
`crates/orzma_vt/src/screen/tests/delete_characters.rs`, declared in
`crates/orzma_vt/src/screen/tests.rs`. No existing module exercises DCH. The
module needs `use crate::screen::grid::run::Style;` beside its `use super::*;`
for TC-02.

## TC-01 — the gap closes and a blank appears at the right margin

| | |
| - | - |
| Setup | `screen()` (4×3); row 0 seeded `a b c d`; cursor at column 1 |
| Act | `screen.delete_characters(1)` |
| Expect | row 0 reads `a`, `c`, `d`, blank **[D2]** ／ the blank sits in the last column, the right margin **[D4]** ／ the cursor stays at column 1 **[P-CURSOR]** ／ returns `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

A full four-column row makes both halves of the operation visible in one
assertion: two cells move left past the cursor column, and exactly one column at
the far end becomes blank. A shift written right-to-left over the overlapping
range duplicates `b` instead of dropping it, and still leaves the last column
blank, so the middle of the row is where that mistake shows.

```rust
/// Asserts that a delete closes the gap by moving the cells right of the
/// cursor left, and blanks the column that opens at the right margin,
/// leaving the cursor where it was.
///
/// Case: a shell's line editor deletes a character from the middle of a
/// command that fills the row.
#[test]
fn a_delete_closes_the_gap_and_blanks_the_right_margin() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', 'c', 'd', blank]
    );
    assert_eq!(screen.state.column, GridColumn(1));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

## TC-02 — the shifted cells keep their attributes; the new blank takes the pen background

| | |
| - | - |
| Setup | `screen()`; pen set to `Style::BOLD` and bg `Indexed(1)`, then `a b c d` printed with it; pen bg changed to `Indexed(4)` with `BOLD` still set; cursor at column 1 |
| Act | `screen.delete_characters(1)` |
| Expect | column 1 holds `c` still carrying `BOLD` and bg `Indexed(1)` **[D3]** ／ column 3 holds a space with `Style::empty()` **[D4]** ／ column 3 carries bg `Indexed(4)` **[P-FILL]** |

DCH is the one place the manual states the attribute rule in both directions —
moved cells keep theirs (D3), and the created blank has none (D4) — so this case
pins them together. As with ICH, "no visual character attributes" reaches the
style bits only: VT510's Table 5-16 and VT220 §4.9.1 enumerate bold, underline,
blinking, negative and invisible and no color, so the background comes from
`Pen::erase_cell` (`crates/orzma_vt/src/screen/cell.rs:86`) instead. Printing
with one pen and deleting with another is what separates an implementation that
copies the old right-edge cell's attributes from one that writes the erase cell.

```rust
/// Asserts that a delete carries each shifted cell's attributes with it
/// while the blank opening at the right margin takes the pen background
/// and none of the pen's rendition.
///
/// Case: a TUI drawing on a colored background deletes a character from
/// text it had already drawn bold.
#[test]
fn the_shifted_cells_keep_their_attributes_and_the_new_blank_takes_the_pen_background() {
    let mut screen = screen();
    screen.pen_mut().style = Style::BOLD;
    screen.pen_mut().bg = Color::Indexed(1);
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c);
    }
    screen.pen_mut().bg = Color::Indexed(4);
    screen.state.column = GridColumn(1);
    screen.delete_characters(1);
    assert_eq!(screen.grid[ScreenLine(0)][1].c, 'c');
    assert_eq!(screen.grid[ScreenLine(0)][1].style, Style::BOLD);
    assert_eq!(screen.grid[ScreenLine(0)][1].bg, Color::Indexed(1));
    assert_eq!(screen.grid[ScreenLine(0)][3].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][3].style, Style::empty());
    assert_eq!(screen.grid[ScreenLine(0)][3].bg, Color::Indexed(4));
}
```

## TC-03 — a count above one deletes that many and blanks as many columns

| | |
| - | - |
| Setup | `screen()`; row 0 seeded `a b c d`; cursor at column 0 |
| Act | `screen.delete_characters(2)` |
| Expect | row 0 reads `c`, `d`, blank, blank **[D2]** ／ two columns became blank, one per deleted character **[D8]** ／ the cursor stays at column 0 **[P-CURSOR]** ／ returns `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

VT220 states the blank count explicitly — one space per deleted character — which
is the part a single-column loop gets wrong when it re-reads its source from the
row it has already shifted. Deleting from column 0 keeps the arithmetic
unambiguous: every surviving cell moves by the same two columns.

```rust
/// Asserts that a count above one deletes that many characters in a
/// single call and blanks one column per deleted character.
///
/// Case: an editor removes a two-column indent guide from the start of a
/// row it is repainting.
#[test]
fn a_count_above_one_deletes_that_many_and_blanks_as_many_columns() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(0);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_characters(2);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['c', 'd', blank, blank]
    );
    assert_eq!(screen.state.column, GridColumn(0));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

## TC-04 — a count past the row deletes only the remaining characters

| | |
| - | - |
| Setup | `screen()`; row 0 seeded `a b c d`; cursor at column 2 |
| Act | `screen.delete_characters(9)` |
| Expect | row 0 reads `a`, `b`, blank, blank **[D6]** ／ the cursor stays at column 2 **[P-CURSOR]** ／ returns `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

D6 states this clamp outright, which is why the case is fully cited here and was
premise-backed on the ICH side. Nine overshoots a four-column row by more than
its width, so an unclamped implementation cannot land back inside the slice by
luck; this is the case that panics rather than failing an assertion when the
count is not bounded by `cols - column`.

```rust
/// Asserts that a count larger than the characters left in the row
/// deletes only those and blanks the columns they vacated.
///
/// Case: an application asks to delete more columns than the row has
/// left while the cursor sits near the right edge.
#[test]
fn a_count_past_the_row_deletes_only_the_remaining_characters() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(2);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_characters(9);
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

## TC-05 — a delete in the last column blanks that cell alone

| | |
| - | - |
| Setup | `screen()`; row 0 seeded `a b c d`; cursor at column 3, the last one |
| Act | `screen.delete_characters(1)` |
| Expect | row 0 reads `a`, `b`, `c`, blank **[D2]**, the blank being the right-margin column the delete vacated **[D4]** ／ the cursor stays at column 3 **[P-CURSOR]** ／ returns `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

The boundary where the cursor column and the right margin are the same cell, so
there is nothing to shift and the deleted cell is also the blanked one. A
length computed as `cols - column - 1` is zero here and correct; written as an
unchecked subtraction elsewhere in the expression it underflows.

```rust
/// Asserts that a delete in the last column blanks that one cell and
/// leaves every column before it untouched.
///
/// Case: the cursor rests on the final column of a full row when the
/// application deletes the character under it.
#[test]
fn a_delete_in_the_last_column_blanks_that_cell_alone() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(3);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_characters(1);
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

## TC-07 — a delete outside the scroll region still closes the gap

| | |
| - | - |
| Setup | `tall_screen()` (4×4); margins `top: ScreenLine(1)`, `bottom: ScreenLine(2)`; row 3 seeded `a b c d`; cursor on line 3, below the bottom margin, at column 1 |
| Act | `screen.delete_characters(1)` |
| Expect | row 3 reads `a`, `c`, `d`, blank **[P-MARGIN]** ／ returns `Some(DamageSpan::rows(ViewportLine(3), ViewportLine(3)))` **[Damage]** |

The same deliberate divergence the ICH run settled, and the appendix records it
with both positions. VT510 says DCH has no effect outside the scrolling margins
(D5, verified); xterm's `DeleteChar` carries no `top_marg` / `bot_marg` test and
gates only on the column margins. The cursor goes below the bottom margin rather
than above the top one because that is the shape applications produce — a status
row pinned under a scrolling pane. `delete_lines` next door *does* carry the
vertical guard, so an implementation copied from it fails exactly this case.

```rust
/// Asserts that a delete applies on the cursor's row even when that row
/// lies outside the scroll region, following xterm rather than VT510's
/// "no effect outside the scrolling margins".
///
/// Case: a full-screen application parks its cursor on a status row
/// below the region it scrolls and edits that row in place.
#[test]
fn a_delete_outside_the_scroll_region_still_closes_the_gap() {
    let mut screen = tall_screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(2),
    });
    seed_row(&mut screen, ScreenLine(3), &['a', 'b', 'c', 'd']);
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.delete_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(3)),
        vec!['a', 'c', 'd', blank]
    );
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(3), ViewportLine(3)))
    );
}
```

## TC-06 — a zero count deletes nothing

| | |
| - | - |
| Setup | `screen()`; row 0 seeded `a b c d`; cursor at column 1 |
| Act | `screen.delete_characters(0)` |
| Expect | row 0 is unchanged **[P-ZERO]** ／ returns `None` **[Damage]** |

`repeat_count` (`crates/orzma_vt/src/interpreter.rs:676`) resolves a zero or
omitted CSI parameter to one before `Screen` is reached, so a zero arriving here
comes from inside the crate and means what it says. Pinning it keeps a later
refactor from moving the default down a layer and silently deleting a character
on every zero.

```rust
/// Asserts that a zero count deletes nothing and reports no damage, the
/// CSI layer's `0 → 1` default having already been applied before
/// `Screen` is called.
///
/// Case: a caller inside the crate passes a count it computed as zero.
#[test]
fn a_zero_count_deletes_nothing() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    let damage = screen.delete_characters(0);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'd']);
    assert_eq!(damage, None);
}
```

## TC-08 — a delete disarms the deferred wrap

| | |
| - | - |
| Setup | `screen()`; `a b c d` printed, which leaves the cursor on column 3 with the deferred wrap armed |
| Act | `screen.delete_characters(1)`, then `screen.print('x')` |
| Expect | `state.pending_wrap` is false after the delete **[P-WRAP]** ／ the `x` lands on row 0, column 3, rather than wrapping to row 1 **[P-WRAP]** |

No manual mentions the deferred wrap — it is xterm's `ResetWrap`, which
`DeleteChar` calls. The second assertion carries the case: clearing the flag
without that check would pass against an implementation that clears it and lets
the next `print` wrap anyway, and `erase_in_line` in this same file takes the
opposite position on an armed wrap (`crates/orzma_vt/src/screen.rs:648`).

```rust
/// Asserts that a delete disarms the deferred wrap, so the next printed
/// character stays on the cursor's row.
///
/// Case: an application fills the last column of a row and then deletes
/// a character before printing again.
#[test]
fn a_delete_disarms_the_deferred_wrap() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c);
    }
    assert!(screen.state.pending_wrap);
    screen.delete_characters(1);
    assert!(!screen.state.pending_wrap);
    screen.print('x');
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'x');
}
```

## TC-09 — a delete below a scrolled viewport reports no damage

| | |
| - | - |
| Setup | `screen()`; three `line_feed()`s from the bottom row to grow history; `viewport.offset = DisplayOffset(3)`; row 0 seeded `a b c d`; cursor on line 0, column 1 |
| Act | `screen.delete_characters(1)` |
| Expect | the grid row still closes the gap to `a`, `c`, `d`, blank **[D2]** ／ returns `None` **[Damage]** |

The one path where content changes and nothing is reported. Three line feeds put
three rows in history, and an offset of three pushes every live row past the
bottom of a three-row viewport, so `damage_span` sees a first row at or beyond
`rows` and yields `None`. The glyph assertion keeps the case honest: an
implementation that returns `None` by skipping the work would otherwise pass.

```rust
/// Asserts that a delete on a row the viewport has scrolled past still
/// closes the gap while reporting no damage.
///
/// Case: the user is reading scrollback when a program edits a line on
/// the live screen below the view.
#[test]
fn a_delete_below_a_scrolled_viewport_reports_no_damage() {
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
    let damage = screen.delete_characters(1);
    assert_eq!(
        row_glyphs(&screen, ScreenLine(0)),
        vec!['a', 'c', 'd', blank]
    );
    assert_eq!(damage, None);
}
```

## Appendix

### Contract table

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| D1 | DCH | unconditional | "This control function deletes one or more characters from the cursor position to the right" | vt510.pdf p.121, L4390 |
| D2 | DCH | unconditional | "As characters are deleted, the remaining characters between the cursor and right margin move to the left" | vt510.pdf p.121, L4403-4404 |
| D3 | DCH | unconditional | "Character attributes move with the characters" | vt510.pdf p.121, L4404 |
| D4 | DCH | unconditional | "The terminal adds blank spaces with no visual character attributes at the right margin" | vt510.pdf p.121, L4404-4405 |
| D5 | DCH | conditional | "DCH has no effect outside the scrolling margins" | vt510.pdf p.121, L4405 |
| D6 | DCH | bounded value | "If Pn is greater than the number of characters between the cursor and the right margin, then DCH only deletes the remaining characters" | vt510.pdf p.121, L4398-4399 |
| D7 | DCH | numeric parameter | "Pn is the number of characters to delete" | vt510.pdf p.121, L4397-4398 |
| D8 | DCH | unconditional | "This creates a space character at the right margin for each character deleted" | vt220.pdf p.35, L1731-1733 |
| D9 | DCH | unconditional | "The spaces created at the end of the line have all their character attributes off" | vt220.pdf p.35, L1734-1736 |
| D10 | DCH | numeric parameter | "Default: Pn = 1" | vt510.pdf p.121, L4400 |
| D11 | DCH | unconditional | "Deletes Pn characters starting with the character at the cursor position" | vt220.pdf p.35, L1728-1730 |
| E2 | DCH | unconditional | "The resulting gap is closed by shifting the contents of the adjacent character positions towards the active presentation position" | ECMA-48.pdf p.52, L2323-2325 |

D1, D9, D11 and E2 restate from a lower-precedence manual or in weaker terms
what D2, D4 and D6 already state, and produce no case of their own. D10 governs
the CSI parameter, which `repeat_count` resolves before `Screen` is called;
TC-06 pins the layer below it instead. D5 is the deliberate divergence below.

D7 was first recorded against the span L4397-4400 and `verify` **rejected** it
for a dropped qualifier — `only`, from D6's sentence sitting between the two
halves of the quote. It was re-recorded as two narrower citations, D7 and D10,
and both were verified again.

### Terms tried

| Term | Level | Reached |
| - | - | - |
| `DCH` | 1 | vt510 L4389 and L4403-4405 (definition), vt220 L1729 (definition), ECMA-48 L2316 (definition). Rejected first: vt510 L192 (contents), L3147 (summary table, no behaviour), L6639 (cross-reference); ECMA-48 L286 (contents), L953 and L2020-area (index), L1545/L1622/L1779/L1799 (cross-references) |
| `CSI Pn P` | 1 | vt510 L4393 (format block), vt220 L1729 |
| "Delete Character" | 1, 4 | the same three definitions |
| "blank spaces", "right margin" | 1 | vt510 L4404-4405, vt220 L1731-1736 |
| "character attributes" | 1 | vt510 L4404 (D3), and Table 5-16 at L9663-9678, which is what bounds D4 to the style bits |
| `Screen`, cell storage, write cursor | 3 | the file's `//!` header names no control function |
| `GridSize`, nonzero axes | 2 | the `Screen` doc's invariant; no control function |
| `u16` count → "Pn", "number of characters" | 4 | vt510 L4397-4400 |
| "cursor position" | 1, 4 | reached D1 and D11, neither of which says whether the cursor moves — see below |
| `Option<DamageSpan>` | 4 | absent from all three manuals, as expected — orzma's own contract, handled by the Damage table |

Level 1 reached statements of behaviour, so levels 2-4 were collected for the
record rather than to rescue the search.

### Not specified

- **Whether the cursor moves** (`P-CURSOR`). This is the one place DCH is
  weaker than ICH: VT510's ICH entry says "The cursor remains at the beginning
  of the blank characters" and VT220's repeats it, while neither manual's DCH
  entry mentions the cursor at all. ECMA-48 is suggestive rather than
  dispositive — § 8.3.64 moves the active position to the line home position for
  ICH and § 8.3.26 states no move for DCH — but an absence is not a statement.
  The premise rests on the author's decision and on xterm's `DeleteChar`, which
  leaves `cur_col` untouched. It earns a case because `delete_lines`
  (`crates/orzma_vt/src/screen.rs:426`) homes the cursor with
  `carriage_return()`, so the mistake this guards against is one copy-paste away.
- **The background of the created blank** (`P-FILL`). VT510's Table 5-16
  "Visual Character Attribute Values" and VT220 §4.9.1 enumerate bold,
  underline, blinking, negative and invisible, and no color parameter, so D4 and
  D9 reach the style bits and stop. The background follows `Pen::erase_cell`
  (`crates/orzma_vt/src/screen/cell.rs:86`) and the `bce` capability the local
  `xterm-256color` entry advertises.
- **The deferred wrap** (`P-WRAP`). No manual mentions it; xterm's `DeleteChar`
  calls `ResetWrap`.
- **A zero count at the `Screen` layer** (`P-ZERO`). D10 governs the CSI
  parameter only.
- **`DamageSpan`** — orzma's own contract; the Damage decision table maps the
  specification-described change onto it.

`docs/references/xterm-ctlseqs.pdf` was read for this run and settles none of
these: its DCH entry is one line — "CSI Ps P  Delete Ps Character(s) (default =
1) (DCH)." (p.13, L664) — stating nothing about attributes, margins, the cursor,
or the wrap. The xterm behaviours above come from its source, which is not in
`docs/references/` and so is cited as evidence for a premise, never as a manual
citation.

### Specification conflicts

**D5 versus TC-07 — a deliberate divergence, settled by the author.**

- vt510.pdf p.121, L4405: "DCH has no effect outside the scrolling margins."
- xterm `util.c`: `DeleteChar` carries no `top_marg` / `bot_marg` test and
  returns early only on `if (!ScrnIsColInMargins(screen, screen->cur_col))`, a
  column check. orzma implements neither DECLRMM nor DECSLRM, so that check
  spans the whole row and xterm applies DCH on every row.

TC-07 pins xterm's behaviour. The manual statement is not thereby wrong — it is
the DEC terminal's behaviour, which orzma deliberately does not reproduce.
`insert_lines` and `delete_lines` in the same file take the DEC position on the
same question, so the two now differ on purpose. The ICH run settled the
identical question as C5 versus its own TC-07.

### Unsourced suggestions

Remarks only. None of these changed a case.

- `seed_row` and `row_glyphs` are specified in `tdd-screen-insert_characters.md`
  and belong in `screen/tests.rs`, shared by both modules.
- The level-1 doc extractor printed `NODOC` for this method too: the stub's
  multi-line `#[expect(unused_variables, ...)]` defeats the extractor's
  single-line `#[` skip. Moving the attribute above the `///` block — or deleting
  it once the body lands — restores it.
- `Grid` has no in-row shift primitive today; `fill_visible_row_range`
  (`crates/orzma_vt/src/screen/grid.rs:140`) is the closest thing, and ICH and
  DCH want the two directions of one new pair.
- D3 ("character attributes move with the characters") is a property the ICH
  side has to honour as well, where the manual states only that text moves. If
  the two methods end up sharing one primitive, that primitive carries whole
  cells and both statements hold by construction.

### Conflicts with docs/todo

None of API shape. Two notes:

- `vt-conformance-scope.md` §5 already concludes that ICH/DCH need a new in-row
  primitive on `Grid`, and its Tier 1 table records DCH's observed frequency as 0
  in the session it measured.
- `nvim-tree-stale-cells-ech.md` records the same measurement exercise for ECH;
  its capability table lists DCH (`dch`, `dch1`) as advertised and unimplemented,
  which is what this pair of documents exists to close.
