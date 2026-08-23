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
