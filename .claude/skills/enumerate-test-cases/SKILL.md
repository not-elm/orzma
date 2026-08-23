---
name: enumerate-test-cases
description: Enumerates the test cases one orzma_vt method owes, deriving every case from a citation in docs/references/ and having Codex review the result. Use when the user says "テストケースを洗い出して", "enumerate test cases", "/enumerate-test-cases", or asks which cases a VT control-function method needs before writing its tests.
argument-hint: [Screen::method]
allowed-tools: Read, Grep, Glob, Bash(pdftotext:*), Bash(codex:*), Bash(grep:*), Bash(awk:*), Bash(sed:*), Bash(head:*), Bash(wc:*), Bash(tr:*), Bash(mkdir:*), Bash(python3:*), AskUserQuestion
---

# Enumerate test cases for one method

Enumerate the test cases a method owes, from the VT specification rather than
from its implementation, and report the list in the terminal.

`Write` and `Edit` are deliberately absent from `allowed-tools`. This skill
reports; it must not modify the repository. `Bash` is granted because
`pdftotext` and `codex` need it, so the property is strong rather than
airtight — do not use it to write into the repository.

## What this skill will not do

- It does not read the method body. Reading it produces tests that pin
  whatever the code already does, including its bugs, and a terminal
  emulator's bugs read as ordinary code.
- It does not compare against tests that already exist.
- It does not generate test code, and it does not write any file into the
  repository.
- It handles one method per run.

## Output language

Case names, `Case:` paragraphs, and `Setup:` / `Act:` / `Expect:` lines are
written in English, because the author transcribes them into Rust `///` doc
comments and this repository requires in-code comments to be English. The
report's own narration — summary, change log, errors — follows the language
of the conversation.

## Phase 0 — Resolve the method

The argument may be a qualified name (`Screen::line_feed`), a bare method name
(`line_feed`), or a path with a line (`crates/orzma_vt/src/screen.rs:174`).
If no argument was given, ask for one with `AskUserQuestion`.

Resolve with `Grep`. `interpreter.rs` mirrors many of `Screen`'s method names —
`single_shift` matches in three places, `reverse_index` and `print` in two — so:

- A bare name that matches both `Screen` and `interpreter.rs` resolves to the
  inherent method on `Screen`, silently.
- Only a name ambiguous *within* `Screen` produces an `AskUserQuestion` listing
  the candidates.
- No match stops with an error naming the searched paths.

Asking on every mirrored name would put a prompt in front of the majority of
runs.

## Phase 1 — Gate and extract the specification

### 1a. The scope gate

Read the method's doc comment — the contiguous `///` block immediately above
its `fn` line, skipping any `#[...]` attributes. Admit the method when that
block contains a `# Control Functions` section AND at least one control
function named there can be found in `docs/references/` (1b below).

Run this to classify a method:

```bash
awk -v m="<method>" '
  /^[[:space:]]*\/\/\// { doc = doc $0 "\n"; next }
  $0 ~ "fn " m "\\(" { print (doc ~ /# Control Functions/ ? "ADMIT" : "REJECT"); found=1; exit }
  /^[[:space:]]*#\[/ { next }
  { doc = "" }
  END { if (!found) print "NOTFOUND" }
' <file>
```

On `REJECT`, stop and say so plainly: the method has no `# Control Functions`
section, so no public specification governs it, so this skill has no
trustworthy source. Name the method and the condition that failed. Do not
improvise a contract from the signature, and do not fall back to reading the
body.

Methods that reject today include `print` (it handles printable characters,
not a control function), the accessors `grid_size` / `cursor` / `pen_mut` /
`viewport_row`, `Screen::new`, `set_display_offset`, every dispatch point in
`interpreter.rs` (that file has no `# Control Functions` section anywhere), and
orzma's own extensions — OSC 5379 `mount` / `unmount`, the `window.orzma`
back-channel, webview APC handling.

Methods that admit today: `backspace`, `carriage_return`, `line_feed`,
`reverse_index`, `erase_in_line`, `erase_in_display`, `move_forward_tabs`,
`move_backward_tabs`, `set_horizontal_tab_stop`, `edit_tab_stop`,
`reset_tab_stops`, `designate_character_set`, `invoke_character_set`,
`single_shift`, `save_checkpoint`, `restore_checkpoint`.

`save_checkpoint` and `restore_checkpoint` are the best case for this skill:
both are empty bodies today and both name a documented control function
(DECSC, DECRC), so a specification-derived list is the only list anyone can
produce for them.

### 1b. Extract the specifications

`docs/references/` holds `ECMA-48.pdf`, `vt220.pdf`, and `vt510.pdf`. Extract
each once per session into a scratchpad directory, skipping files already
there:

```bash
mkdir -p "$SCRATCH/vt-refs"
for f in ECMA-48 vt220 vt510; do
  [ -f "$SCRATCH/vt-refs/$f.txt" ] || pdftotext -layout "docs/references/$f.pdf" "$SCRATCH/vt-refs/$f.txt"
done
```

All three extract in about 0.6 s together. Search them with `Grep`.

`-layout` is fixed, not incidental: the same PDF yields 13943 lines with
`-layout`, 15183 with `-raw`, and 35996 with neither, so the flag set is part
of any line-number citation.

If `pdftotext` is unavailable, stop and suggest `brew install poppler`. Do not
fall back to `Read`'s PDF page range: with no way to search a 13 MB manual,
the run would cover whichever pages happened to be sampled, and a partial
enumeration is indistinguishable from a complete one.

### 1c. Cite what you find

A citation records four things:

```
<pdf> p.<page>, L<first>-<last> — "<reassembled statement>"
```

- **PDF page index** — computed, never guessed:

  ```bash
  page_of() { head -"$2" "$1" | tr -cd '\f' | wc -c | awk '{print $1+1}'; }
  ```

  A page number written from memory is the same fabricated citation that
  Phase 4 exists to reject, arriving one step earlier and unchecked.

- **Line span** — a session-local hint that makes the quote quick to find. It
  carries no guarantee across poppler versions, so it is never the thing a
  reader is asked to trust.

- **Reassembled statement** — the quote is rarely a clean line. DEC manuals
  state behaviour inside multi-column tables and `-layout` preserves that
  geometry, so a statement arrives split across lines with a neighbouring
  column interleaved:

  ```
  Reverse index        RI            Moves the cursor up one line in the same column. If the cursor is
                       8/13          the top margin, the page scrolls down.
  ```

  The statement is "Moves the cursor up one line in the same column. If the
  cursor is at the top margin, the page scrolls down". `RI` and `8/13` are
  other columns of the same table row. Record the reassembled statement and
  the span it came from.

### 1d. Reject hits that are not definitions

Searching the ECMA-48 extraction for `RI` reaches the table of contents, the
acronym index, and cross-references inside other entries before reaching the
entry itself. Reject a hit when:

- its line ends in a bare page number (contents),
- it sits inside a contents or index table,
- it states no behaviour.

Keep reading hits until one states behaviour. Record how many were rejected.
If every hit for a control function is a contents, index, or cross-reference
line, treat that function as absent from that manual and descend the
precedence order.

### 1e. Precedence among the three manuals

Applied only when they disagree:

1. `vt510.pdf` — the terminal orzma emulates, and the most recent DEC
   statement of any DEC-specific behaviour.
2. `vt220.pdf` — where VT510 is silent, and the source of the naming
   vocabulary this crate already follows.
3. `ECMA-48.pdf` — the general definition of a control function, and the
   fallback where both DEC manuals are silent.

Descending a level requires evidence, not an impression. Before treating VT510
as silent on a control function, try the mnemonic (`RI`), the expanded name
(`Reverse index`), and the escape form (`ESC M`), and record which terms were
tried beside the fallback citation. Otherwise the precedence descends on an
unfalsifiable claim, and a reader cannot tell a thorough search from one failed
grep.

A disagreement that survives this ordering is **not** resolved here. Report it
as a specification conflict, with both citations, and derive no case from it.

### 1f. Look up per control function

A method's doc may name five — `line_feed` names LF, VT, FF, IND, and NEL — and
each gets its own lookup. Control functions named in the doc but absent from
`docs/references/` are dropped individually and listed in the report. Only when
all of them are absent does the run stop.

### 1g. Output: the contract table

| ID | Control function | Shape | Statement | Citation |
| --- | --- | --- | --- | --- |
| C1 | RI | unconditional | The cursor moves up one line in the same column. | vt510.pdf p.64, L2435-2436 |
| C2 | RI | conditional | At the top margin, the page scrolls down instead. | vt510.pdf p.64, L2435-2436 |

`Shape` is one of unconditional, conditional, numeric parameter, bounded value,
mode-dependent, or multi-function. It is the only classification the table
carries, and Phase 2 keys on it directly.

**An entry with no citation does not enter the table.** List entries dropped
for that reason under "Excluded as unspecified" in the report, so the reader
knows what the enumeration does not cover rather than mistaking the list for
complete.

**Do not read the method body.** Read the signature and the types it mentions
(`Damage`, `EraseLineMode`, `GridSize`) — the output has to name real
constructors and real variants, and that is API shape, not behaviour. A
divergence between a spec-derived case and the current code is a finding, not
a mistake to smooth over.

## Phase 2 — Derive the cases

Expand each contract entry by its shape:

| Contract shape | Cases produced |
| --- | --- |
| Unconditional statement (`Moves the cursor down one line`) | one nominal case |
| Conditional (`If the cursor is at the bottom margin, the page scrolls up`) | two cases, condition met and not met, with the boundary itself on the met side |
| Numeric parameter (`Pn — default = 1`) | omitted default, explicit 1, a value above 1, and 0 handled as the spec defines it |
| Bounded value (`Pt must be less than Pb`) | lower bound, upper bound, out of range |
| Mode-dependent behaviour (DECOM, DECAWM, margins set or unset) | one case per mode |
| Several control functions on one method | one case per point where they differ; identical behaviour collapses to one case annotated with the functions it covers |

Merge cases that end up with the same setup, action, and expectation.

The two parameter rows fire less often than they look. `Screen` receives
parameters already decoded — `EraseLineMode`, `CharacterTabEdit`, a plain
`count: u16` — because the `Ps` decode and its defaults live in
`CharacterTabEdit::from_tbc` / `from_ctc` and in the CSI dispatcher, both
outside the gate. At this layer the rows mostly apply to `move_forward_tabs`
and `move_backward_tabs`.

### Return value

Six in-scope methods return `()` — `set_horizontal_tab_stop`, `edit_tab_stop`,
`reset_tab_stops`, `designate_character_set`, `invoke_character_set`,
`single_shift`. For those the expectation covers screen state alone and carries
no return line.

The rest return `Option<Damage>`, which is orzma's own contract; no VT manual
mentions it. Derive it from the spec-described state change:

| Spec-described change | Expected return |
| --- | --- |
| A bounded contiguous span of rows changes contents, and the span is inside the viewport | `Some(Damage::rows(first, last))` |
| Content moves across the whole screen — a scroll, `ED 2`, a reset | `Some(Damage::Full)` |
| Rows changed contents, but all of them sit outside the viewport | `Some(Damage::Metadata)` |
| Only metadata changes — cursor position, pen, deferred-wrap flag | `Some(Damage::Metadata)` |
| Nothing changes | `None` |

`Damage` has three variants, not two: `Full`, `Rows { first, last }`, and
`Metadata` (`crates/orzma_vt/src/damage.rs:152`). Sending every content change
to `Full` gives a wrong expectation for `erase_in_line` and for both partial
`erase_in_display` modes, which return `Damage::rows(..)` and are already
pinned by four existing tests. It also loses the third row: `Metadata` means
"a frame is still needed" and covers changes that landed entirely outside the
viewport, not only cursor motion.

The specification stays the source for *what changes*; this table maps that to
*what is reported*.

### Priority

- **High** — behaviour the spec states outright, and the boundaries of
  conditions it states outright.
- **Medium** — behaviour that follows from the spec but depends on a default
  or a mode.
- **Low** — combinations of modes the spec does not address individually.

Emit cases in that order and say so in the report, so an author who stops after
the High block still has every case the specification states outright.

### Naming

Declarative English sentences with articles, matching `screen.rs`:
`a_reverse_index_at_the_top_margin_scrolls_the_screen_down`.

### Destination

Resolve it; do not assume `mod tests::<method>`. `screen.rs` holds sixteen test
modules and they do not map one-to-one onto methods: `tab_stop_edits` covers
`set_horizontal_tab_stop`, `edit_tab_stop`, and `reset_tab_stops` together, and
the three character-set methods have no module in `screen.rs` at all — their
tests live in `crates/orzma_vt/src/screen/character_sets.rs` under
`mod tests::designate`, `invoke`, and `single_shift`.

Grep the `mod` declarations inside the defining file's `#[cfg(test)] mod tests`
block, pick the module whose tests already exercise the same control function,
and fall back to naming a new module when none does. `save_checkpoint` and
`restore_checkpoint` take that fallback today: they have no tests anywhere.

Report the file as well as the module, because for the three character-set
methods the file is not `screen.rs`.

`Setup:` lines may reach private state such as `margins.top` directly, since
the tests live in the same file as the type.

### The per-case output block

```
### TC-03  a_reverse_index_at_the_top_margin_scrolls_the_screen_down
Control function: RI    Shape: conditional    Priority: High
Source: C2 (vt510.pdf p.64, L2435-2436 — "If the cursor is at the top margin,
        the page scrolls down")

Case:   The top margin sits on the third row and the cursor is on it when RI arrives.
Setup:  Screen::new(GridSize { cols: 80, rows: 24 }, 0); margins.top = ScreenLine(2);
        cursor on ScreenLine(2); each row carries a distinguishable character
Act:    screen.reverse_index()
Expect: the margin region scrolls down one row
        the cursor stays on ScreenLine(2)
        the top margin row holds erase cells
        the deferred wrap is disarmed
        returns Some(Damage::Full), because content moves across the screen
```

The `Source` line is the point of the whole skill: every case names the
sentence that demands it, and **a case that cannot name one is not emitted.**

`Case:` carries the scenario and nothing else — no restatement of the
assertions, no policy, no speculation about how a broken implementation would
misbehave. `.claude/rules/rust.md` governs that paragraph, and the author
transcribes it verbatim. The contract line and any policy paragraph the rule
also requires are the author's to write, because both state a decision the
specification does not make.
