# Neovim scrolling leaves the rows above the scroll direction stale

Status: open. Found 2026-09-10 while running Neovim 0.12.5 inside orzma on
macOS.

## Symptom

Scrolling a file in Neovim repaints only the rows at the scroll edge. In
`docs/todo/bug.png` (a 210x51 window editing `src/cef_profile.rs`) the cursor is at
line 105, yet screen rows 1-46 still show file lines 1-46 and only the
last two rows show lines 104-105. The stale rows also carry scattered
one-cell corruption: a `{` replacing a single character on several rows
(`a{shared`, `PI{ guarantees`, `parent.jo{n`, `&P{th`, `{td::io`) and a
missing first character on two rows (`ub(crate) struct`, `mpl CefProfileDir`).

## Cause

`orzma_vt` does not implement insert line (IL, `CSI Pn L`) or delete line
(DL, `CSI Pn M`). Neovim's TUI scrolls a window with exactly those two
sequences, so the scroll is silently dropped and the grid never moves.

What Neovim emits for every scroll (captured from a PTY on this machine
with the user's config, `TERM=xterm-256color`, screenshot geometry):

```
CSI 3;50 r      DECSTBM: scroll region rows 3-50 (tabline, winbar, 48 text rows, statusline)
CSI 3;1 H       cursor to the top-left of the region
CSI M           DL, default count 1   (CSI n M for n rows; CSI L / CSI n L scrolling back)
CSI r           reset the region
CSI 50;1 H ...  repaint only the newly exposed bottom row(s)
```

Neovim's `tui_grid_scroll` (0.10.4 through master) uses only `csr` plus
`dl1` / `dl` / `il1` / `il`. It never emits IND, RI, SU (`CSI S`), SD
(`CSI T`), HPA, VPA, ICH, DCH, or REP. The complete inventory of CSI
final bytes in a 194 KB capture was `m H h l K r M t c B A C J n u`, and
every one of them is implemented except `M`.

Where it is dropped:

- `crates/orzma_vt/src/interpreter.rs:226-350`: `csi_dispatch` matches
  `H f r c n t A B C D E F J K I Z g W m` plus the private-mode forms and
  ends in `_ => {}` at line 349. There is no `(None, b'L')` or
  `(None, b'M')` arm. The only `M` handler is `ESC M` (RI) at line 189.
- `crates/orzma_vt/src/screen.rs` has no line-editing API (no
  `insert_lines` / `delete_lines` / `scroll_up` / `scroll_down`).
- The gap is known: `crates/orzma_vt/src/interpreter/tests/unsupported_sequences.rs:14`
  and `docs/superpowers/specs/2026-08-31-engine-swap-design.md:123` list
  ICH / DCH / IL / DL / ECH as out of scope for the engine swap.
- `src/main.rs:109-145` forces `TERM=xterm-256color`, whose terminfo
  advertises `dl=\E[%p1%dM`, `dl1=\E[M`, `il=\E[%p1%dL`, `il1=\E[L`, and
  `csr`, so Neovim has no reason to fall back to a full redraw.

The `{` corruption is the same bug, not a second one. Once orzma's screen
diverges from Neovim's model, Neovim's ordinary single-cell incremental
writes (`CUP` plus one styled character, 166 of them in a nine-keystroke
capture, 28 of them a single space) land on stale rows. In the screenshot
the corrupted cells form three vertically adjacent pairs sharing a
column, and one `{` carries an orange bracket-highlight colour the
surrounding text does not, which is the signature of Neovim's model being
one row out of step after each ignored scroll.

## What is not implicated

The pipeline below the interpreter was traced and is sound:

- `Screen::line_feed` / `reverse_index` scroll at the margins and stage
  `DamageSpan::Full` (`screen.rs:372-414`); DECSTBM resolves correctly
  (`screen.rs:680-690`, `screen/margins.rs:126-140`).
- `Damage` merges spans and a pending `Full` supersedes rows
  (`frame/damage.rs`); `FrameTracker::emit` clears only after the frame is
  settled (`frame.rs:139-175`).
- The coalescer in `crates/orzma_tty/src/coalescer.rs` is timing only.
- The backend event channel is unbounded and `drain_orzmux_events`
  forwards every frame.
- `TerminalGrid::apply` replaces exactly the carried rows
  (`crates/orzma_tty_renderer/src/schema/grid.rs:249-285`); `Row::to_runs`
  and `runs_to_cells` are one-to-one for ASCII.

## Fix

1. Add `Screen::insert_lines(count)` and `Screen::delete_lines(count)`
   built on the existing region-aware `Grid::scroll_down_one` /
   `Grid::scroll_up_one` (`screen/grid.rs:153`, `:202`) over the range
   from the cursor line to the bottom margin. A no-op when the cursor is
   outside the DECSTBM region; clamp the count to the remaining region;
   move the cursor column to the left margin; fill with the pen's erase
   cell; stage `DamageSpan::Full`.
2. Add `(None, b'L')` and `(None, b'M')` arms in `csi_dispatch`, default
   count 1.
3. Add SU (`CSI Pn S`) and SD (`CSI Pn T`) at the same time. They share
   the primitives over the full region. Neovim does not use them, but the
   advertised terminfo declares `indn` / `rin`.
4. Tests: IL/DL inside and outside the region, count clamping, cursor
   column reset, and that the emitted frame carries every row of the
   region.

## Related gaps found on the way

Separate follow-ups, not needed for this bug:

- DECRQM (`CSI ? Pd $p`) is never answered because any CSI with an
  intermediate byte is dropped at `interpreter.rs:228-230`. Neovim probes
  modes 69, 2026, 2027, 2031, and 2048 at startup. Answering 2026 as set
  or reset turns on synchronized output; answering 69 the same way makes
  Neovim 0.12+ use DECLRMM / DECSLRM for vertical-split scrolling, so
  replying is a decision about which sequences arrive next.
- DECTCEM (`CSI ? 25 h/l`) is unimplemented; Neovim sends it around every
  flush.
- Wide characters: the VT advances one column per scalar while the
  renderer advances by display width (TODO at `screen.rs:165-169`).

## Sources

- `bug.png`, and PTY captures of Neovim 0.12.5 taken on 2026-09-10.
- Neovim `src/nvim/tui/tui.c` (`tui_grid_scroll`, `set_scroll_region`,
  `terminfo_start`, `tui_handle_term_mode`) at v0.10.4, v0.11.0, v0.12.0,
  and master.
- `infocmp -1 -x xterm-256color` (macOS ncurses 6.0) and upstream
  ncurses `terminfo.src`.
