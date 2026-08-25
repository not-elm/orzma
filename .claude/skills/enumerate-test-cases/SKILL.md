---
name: enumerate-test-cases
description: Enumerates the test cases one orzma_vt method owes, deriving every case from a citation in docs/references/ and verifying each citation before emitting it. Negotiates with the author when a verified citation cannot become a case under the method's current signature. Use when the user says "テストケースを洗い出して", "enumerate test cases", "/enumerate-test-cases", or asks which cases a VT method needs before writing its tests.
argument-hint: [Screen::method]
allowed-tools: Read, Grep, Glob, Bash(pdftotext:*), Bash(grep:*), Bash(awk:*), Bash(sed:*), Bash(head:*), Bash(wc:*), Bash(tr:*), Bash(mkdir:*), Bash(python3:*), AskUserQuestion
---

# Enumerate test cases for one method

Enumerate the test cases a method owes, from the VT specification rather than
from its implementation, and report the list in the terminal.

`Write` and `Edit` are deliberately absent from `allowed-tools`. This skill
reports; it must not modify the repository. `Bash` is granted because
`pdftotext` needs it, so the property is strong rather than
airtight — do not use it to write into the repository.

## What this skill will not do

- It does not read the method body. Reading it produces tests that pin
  whatever the code already does, including its bugs, and a terminal
  emulator's bugs read as ordinary code.
- It does not compare against tests that already exist.
- It does not generate test code, and it does not write any file into the
  repository.
- It does not change the method it enumerates. Phase 4 can settle with the
  author that a signature is wrong; the agreement becomes a premise for this
  enumeration and a record in the report, and the edit stays the author's.
- It handles one method per run.

## Output language

Case names, `Case:` paragraphs, and `Setup:` / `Act:` / `Expect:` lines are
written in English, because the author transcribes them into Rust `///` doc
comments and this repository requires in-code comments to be English. The
report's own narration — summary, change log, errors — follows the language
of the conversation.

## Shell conventions

Two things hold for every `Bash` recipe below.

`$SCRATCH` is this session's scratchpad directory, the one the system prompt
names, which lives outside the repository. Set it yourself before the first
recipe runs — `SCRATCH=<that path>` — or substitute the literal path wherever
the recipes write `$SCRATCH`. No recipe here writes inside the repository.

`page_of` and `verify` are shell functions, and the `Bash` tool starts a fresh
shell for every call, so no function, variable, or working directory survives
from the previous one. Redefine `SCRATCH` and whichever function you need
inside each invocation that uses it. A call that relies on a definition from an
earlier call fails with `command not found`, and a `verify` that never ran is
worse than one that failed.

## Phase 0 — Resolve the method

The argument may be a qualified name (`Screen::line_feed`), a bare method name
(`line_feed`), or a path with a line (`crates/orzma_vt/src/screen.rs:174`).
If no argument was given, ask for one with `AskUserQuestion`.

Resolve with `Grep`. `interpreter.rs` mirrors four of `Screen`'s method names —
`invoke_character_set`, `print`, `reverse_index`, and `single_shift` — so a bare
name can land in more than one file: `single_shift` matches in three places,
`reverse_index` and `print` in two. Therefore:

- A bare name that matches both `Screen` and `interpreter.rs` resolves to the
  inherent method on `Screen`, silently.
- Only a name ambiguous *within* `Screen` produces an `AskUserQuestion` listing
  the candidates.
- No match stops with an error naming the searched paths.

Asking on every mirrored name would put a prompt in front of the majority of
runs.

## Phase 1 — Find and extract the specification

### 1a. Collect the search terms

There is no scope gate. Every method proceeds to 1b, and a method the manuals
do not govern produces a report with no cases rather than a refusal. A
`# Control Functions` section is a convenience where one exists, not a
condition of admission: `ScrollRegion::scroll_span` names `DECSTBM` in prose
and nowhere else, and DECSTBM governs it exactly as much either way.

What that section did supply was the search terms, feeding 1f directly.
Collect them from four places instead. Exhaust every term a level offers, and
descend only when none of them reached a statement of behaviour (1b–1d) — a
level whose terms all dead-end is a level that yielded nothing.

| Level | Source | `scroll_span` yields |
| --- | --- | --- |
| 1 | The method's whole `///` block, section or prose | `DECSTBM`, `OriginMode` |
| 2 | The doc of the enclosing `struct` / `enum` / `impl` | `DECSTBM`, `DECOM` |
| 3 | The file's `//!` header | `DECSTBM`, `DECOM` |
| 4 | The signature's type names and the method name, expanded to specification vocabulary | "scrolling region", "top margin", "bottom margin" |

Read level 1 with this extractor rather than a generic file read with a
guessed line range. A `Read` call bounded by an offset and a limit does not
know where the `///` block ends, and a plausible-looking limit can run past it
into the very body this skill forbids reading:

```bash
awk -v m="<method>" '
  /^[[:space:]]*\/\/\// { doc = doc $0 "\n"; next }
  $0 ~ "fn " m "\\(" { printf "%s", (doc == "" ? "NODOC\n" : doc); found = 1; exit }
  /^[[:space:]]*#\[/ { next }
  { doc = "" }
  END { if (!found) print "NOTFOUND" }
' <file>
```

This prints exactly the accumulated `///` block and stops before the `fn`
line, so it cannot show a line of the body. Its two markers mean different
things, and neither is decoration.

**`NOTFOUND` means stop and report the extraction failure.** The `fn` line was
never reached, so the method is not in the file you passed. **It never means
fall back to a `Read` with a guessed offset and limit** — that unbounded read
is the exact failure this extractor exists to prevent, and it lands in the
method body this skill must not see. Report which file and method name were
tried, and let the author correct them.

**`NODOC` means the method has no doc comment.** That is an ordinary input
now, not a failure: descend to level 2 and carry on. One caveat earns it a
second look first. A doc block separated from its `fn` by a **multi-line**
attribute also arrives empty, because the single-line `#\[` skip cannot span
it and the `doc = ""` rule wipes the block. Treat a `NODOC` on a method you
expected to carry docs as that case until you have checked.

Levels 2 and 3 share one recipe, which emits doc lines only and so cannot
reach a method body:

```bash
grep -n '^[[:space:]]*\(///\|//!\)' <file>
```

It prints every doc line in the file, the `#[cfg(test)]` ones included. Take
the `//!` header and the block above the enclosing item, and leave the rest:
a test's `Case:` paragraph is one author's scenario, not a specification term,
and seeding a lookup from it searches the manuals for the tests you already
have.

Level 4 is the only level carrying judgement, so it is bounded: it maps
identifiers to specification vocabulary and does nothing else. **It never
invents a control function the code does not name.** `Margins` reaching "top
margin" is the expansion the level exists for; `Margins` reaching DECSLRM
because left and right margins also exist is the failure it forbids.

Record every term tried and the level it came from, and report both under
"Terms tried". No grep verdict stands behind a refusal any more, so that
record is the only thing making "searched and found nothing" falsifiable —
the same reasoning 1e already applies to descending the manual precedence.

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
  Phase 3 exists to reject, arriving one step earlier and unchecked.

- **Line span** — a session-local hint that makes the quote quick to find. It
  carries no guarantee across poppler versions, so it is never the thing a
  reader is asked to trust.

- **Reassembled statement** — the quote is rarely a clean line. DEC manuals
  state behaviour inside multi-column tables and `-layout` preserves that
  geometry, so a statement arrives split across lines with a neighbouring
  column interleaved:

  ```
  Reverse index        RI            Moves the cursor up one line in the same column. If the cursor is at
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

### 1f. Look up per term

A level may offer five — `line_feed` names LF, VT, FF, IND, and NEL — and each
gets its own lookup. Terms that reach nothing in `docs/references/` are dropped
individually and listed under "Terms tried". When no term on any level reaches
a statement of behaviour, the run still reports: 0 cases, carrying every term
tried. That is an ordinary outcome, not an error, and not a refusal.

### 1g. Output: the contract table

| ID | Governs | Shape | Statement | Citation |
| --- | --- | --- | --- | --- |
| C1 | RI | unconditional | The cursor moves up one line in the same column. | vt510.pdf p.64, L2435-2436 |
| C2 | RI | conditional | At the top margin, the page scrolls down instead. | vt510.pdf p.64, L2435-2436 |

`Governs` holds a mnemonic where one exists (`RI`, `DECSTBM`) and the
specification section's title where none does. It is not restricted to control
functions, because the method under enumeration need not implement one.

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
`CharacterTabEdit::from_tbc` / `from_ctc` and in the CSI dispatcher, both of
which run before `Screen` sees the call. At this layer the rows mostly apply to
`move_forward_tabs` and `move_backward_tabs`.

### Return value

Most methods return `()` — `set_horizontal_tab_stop`, `edit_tab_stop`,
`reset_tab_stops`, `designate_character_set`, `invoke_character_set`,
`single_shift`, `save_checkpoint`, `restore_checkpoint`, `backspace`,
`carriage_return`, `move_forward_tabs`, `move_backward_tabs`,
`move_cursor_to`, `set_scroll_region`, and `set_origin_mode`. For those
the expectation covers screen state alone and carries no return line;
pure cursor motion reaches the renderer through the per-chunk cursor
diff, not through a return value.

The rest return `Option<Damage>`, which is orzma's own contract; no VT
manual mentions it. Derive it from the spec-described state change:

| Spec-described change | Expected return |
| --- | --- |
| A bounded contiguous span of rows changes contents, and the span is inside the viewport | `Some(Damage::rows(first, last))` |
| Content moves across the whole screen — a scroll, `ED 2`, a reset | `Some(Damage::Full)` |
| Rows changed contents, but all of them sit outside the viewport | `None` |
| Nothing on screen changes — cursor position, pen, deferred-wrap flag | `None` |

`Damage` has exactly two variants: `Full` and `Rows { first, last }`
(`crates/orzma_vt/src/damage.rs:160`). Sending every content change
to `Full` gives a wrong expectation for `erase_in_line` and for both partial
`erase_in_display` modes, which return `Damage::rows(..)` and are already
pinned by four existing tests. There is no metadata variant: a change no
viewport row shows reports nothing, and the emit layer's own diffs decide
whether a frame is still owed.

The specification stays the source for *what changes*; this table maps that to
*what is reported*.

A return type outside those two shapes takes neither branch.
`ScrollRegion::scroll_span` returns `RangeInclusive<ScreenLine>`, and no
`Damage` reaches its caller at all. Derive the expected value from the
specification-described state directly and have the `Expect:` line name the
concrete value — the inclusive range from the top margin to the bottom margin,
for that method. The Damage table does not apply, and neither does the
`Expect:` source tag that cites it.

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
`set_horizontal_tab_stop`, `edit_tab_stop`, and `reset_tab_stops` together.

Grep the `mod` declarations inside the defining file's `#[cfg(test)] mod tests`
block, pick the module whose tests already exercise the same control function,
and fall back to naming a new module when none does. Five methods take that
fallback today, because they have no tests anywhere: `save_checkpoint`,
`restore_checkpoint`, `designate_character_set`, `invoke_character_set`, and
`single_shift`.

The last three carry a trap worth naming, because a module list alone walks
into it. `crates/orzma_vt/src/screen/character_sets.rs` does contain
`mod tests::designate`, `mod tests::invoke`, and `mod tests::single_shift`, and
the names line up almost exactly with the three `Screen` methods. They are not
their tests: they exercise `CharacterSetMapping::{designate, invoke,
single_shift}`, the mapping type that `Screen` delegates to. `Screen`'s own
three methods are untested — grep finds each of them only at its definition in
`screen.rs` and at its `interpreter.rs` call sites. Match the destination on
the type under test, not on the module name.

Report the file as well as the module. A module name does not say which file
holds it, `orzma_vt` splits `Screen` across `screen.rs` and `screen/`, and the
character-set case above shows two files can offer the same module name for
different types.

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
Expect: the margin region scrolls down one row            [C2]
        the cursor stays on ScreenLine(2)                 [C1 — "in the same column"]
        the top margin row holds erase cells              [C2 — "the page scrolls down"]
        the deferred wrap is disarmed                     [doc comment]
        returns Some(Damage::Full)                        [Damage table — content moves
                                                           across the screen]
```

The `Source` line is the point of the whole skill: every case names the
sentence that demands it, and **a case that cannot name one is not emitted.**

That rule binds **per `Expect:` line, not per case**. Every `Expect:` line ends
with its own source in brackets, and there are exactly three legitimate ones: a
contract-table ID (`C1`, `C2`, …), the method's doc comment, or the Damage
decision table in Phase 2 — the last available only to a method that returns
`Option<Damage>`. An `Expect:` line that can name none of the three is dropped,
the same way an uncited case is.

Binding the rule to the case instead would let an unsourced assertion ride
inside a case whose header cites a real sentence, and the reader has no way to
tell it apart from the sourced lines around it. That is the shape an
implementation-derived expectation takes when it enters a specification-derived
list: not a fabricated citation, but a well-cited case carrying one extra line
nobody asked the manual about. Tagging each line makes the unsourced assertion
visible instead of inferred.

`Case:` carries the scenario and nothing else — no restatement of the
assertions, no policy, no speculation about how a broken implementation would
misbehave. `.claude/rules/rust.md` governs that paragraph, and the author
transcribes it verbatim. The contract line and any policy paragraph the rule
also requires are the author's to write, because both state a decision the
specification does not make.

### When a case cannot be written

An entry sometimes arrives here carrying a statement of behaviour and no way
to become a case, because the signature cannot say what the statement
demands. Dropping it hides a specification the code does not serve, and
writing the case anyway produces a `Setup:` or `Expect:` line the author
cannot compile. Record it instead as a **blocker**, carrying its entry ID, its
citation, and the exact line that could not be written.

| Kind | The entry cannot become a case because |
| --- | --- |
| **R** — return contradiction | The return type, or the return contract the doc comment states, disagrees with what the specification-described state change demands. A doc comment promising that nothing is reported when the screen is already clear contradicts the Damage table's row for content moving across the whole screen, and no manual states the optimisation it describes. |
| **P** — missing parameter | The specification states a condition, and no parameter lets a `Setup:` line establish it. |
| **O** — unobservable effect | The specification states a state change, and neither the return value nor any state a test can reach lets an `Expect:` line observe it. |
| **T** — parameter type mismatch | The parameter type rejects a value the specification gives meaning to, or admits one the specification forbids. |

`save_checkpoint` is the standing **O**. `vt510.pdf` has DECSC save the state
of origin mode (DECOM) and the selective erase attribute, both quotes verify
against the DECSC description list, and `Screen` models neither, so no
`Setup:` can establish that state and no `Expect:` can observe it.
`line_feed`'s LNM-set branch is another. On the acceptance run this covered 2
of `save_checkpoint`'s 7 contract entries, so it is a common outcome rather
than an edge case.

A blocker stands on exactly one thing: **a citation that cannot become a
case.** An impression that the signature looks wrong is not a blocker, and
neither is a preference about how the method ought to be shaped. Every claim
this skill makes names a sentence in a manual, and a gate that opens on taste
is the one hole through which unsourced design walks into the list.

A blocker goes to Phase 3 before it goes anywhere else. Phase 4 settles it on
the strength of its citation and on nothing else, so a blocker whose citation
fails verification was never a blocker.

## Phase 3 — Verify every citation

Verify **every** citation the run recorded, not a sample — the ones behind
cases you are about to emit and the ones behind blockers alike. This is the
only check standing between a plausible-sounding sentence and a case the
author will trust, and a fabricated citation reads exactly like a real one —
that is what makes it dangerous. You are verifying your own work, which is
precisely the situation where skipping the check feels safest and is least
safe.

### Verifying a citation

Normalize the quote and the cited span the same way — collapse all whitespace —
and accept the quote when its content words appear as an ordered subsequence of
the span, tolerating tokens from neighbouring table columns. Two guards bound
that tolerance, because a bare subsequence test is far weaker than it looks:

```bash
verify() {
  local file=$1 first=$2 last=$3 quote=$4
  local width=$(( last - first + 1 ))
  if [ "$width" -gt 10 ]; then
    echo "SPAN TOO WIDE ($width lines; the bound is 10)"
    return
  fi
  sed -n "${first},${last}p" "$file" | tr -s '[:space:]' ' ' | tr -d '\n' > "$SCRATCH/_span.txt"
  python3 - "$quote" "$SCRATCH/_span.txt" <<'PY'
import sys, re
quote, spanfile = sys.argv[1], sys.argv[2]
span = open(spanfile).read()
qt = re.findall(r"[A-Za-z0-9]+", quote.lower())
st = re.findall(r"[A-Za-z0-9]+", span.lower())
STOP = {"not", "no", "never", "cannot", "unless", "except",
        "only", "must", "always", "before", "after"}
i, started, skipped = 0, False, []
for t in st:
    if i < len(qt) and t == qt[i]:
        started, i = True, i + 1
    elif started and i < len(qt):
        skipped.append(t)
if i < len(qt):
    print(f"REJECTED ({i}/{len(qt)} matched)")
else:
    dropped = sorted({t for t in skipped if t in STOP})
    print(f"REJECTED (dropped qualifier: {', '.join(dropped)})" if dropped else "VERIFIED")
PY
}
```

**The dropped-qualifier guard.** A subsequence test passes on *any* subsequence
of the span, so it cannot see a word the quote left out — including the word
that carries the meaning. Against `vt510.pdf` L1353, which reads "Panning does
**not** occur until the input buffer becomes empty and the cursor is
displayed", the plain subsequence check returns `VERIFIED` for the quote with
`not` and `VERIFIED` for the quote without it, approving a citation whose
meaning is inverted. So after the subsequence match succeeds, the span tokens
that were skipped inside the matched window are scanned, and a skipped token
from the stop-list rejects the citation and names the word. `not`, `never`,
`cannot`, `unless`, and `only` are the load-bearing entries; the rest are
cheap to keep.

A rejection naming a word your sentence never contained usually means the span
is loose rather than the quote wrong — the skipped word came from an
intervening line or a neighbouring column. Narrow the span until it covers the
one statement you are quoting, then re-run. Never widen the quote to swallow a
word from another column; that records text the sentence does not contain.

**The span-width bound.** You choose both the span and the quote, so nothing
except this bound stops the two from being chosen to fit each other. The check
degrades as the span grows — it holds at 2 to 500 lines, weakens past roughly
2000, and against the whole 13943-line file a wholly invented sentence
assembled from common words verifies, because every one of its words appears
somewhere in that order. A span wider than 10 lines is therefore refused
outright rather than checked. `SPAN TOO WIDE` is not a verification result:
narrow the citation to the lines that carry the statement and run `verify`
again. A real table-row citation never needs more than a few lines.

Do **not** use a literal `grep`. A literal search for the genuine RI statement
returns zero hits against the file it was taken from, because the table splits
it and injects `8/13` into the middle. That strict a check rejects nearly every
citation these manuals can produce.

Neither guard makes `verify` a proof. It still cannot see an ordinary word
dropped from the middle of a sentence, so a truncated quote can pass. Read the
span you cited; `verify` catches the failures that survive a careless read, not
the ones that survive no read at all.

### What to do with each result

**VERIFIED** — the contract entry stands, and the cases derived from it are
emitted.

**REJECTED** — the citation does not say what you recorded. **Do not edit the
quote until it passes.** That is fitting the evidence to the claim, and it
turns the one honest check in this skill into a rubber stamp. Go back to the
extraction, read what the manual actually says, and either re-record the
contract entry against the real text or drop it.

**A re-recorded entry is not verified until `verify` has run on it again.** The
re-recording changes the quote, the span, or both, so the earlier result says
nothing about the new pair, and an entry that reaches the report on the
strength of a run against text it no longer carries is exactly the unchecked
citation this phase exists to catch. Re-run, and count it in `citations
verified: N/N` only once it passes.

A dropped entry goes under "Excluded as unspecified" with the quote that
failed and its match ratio, so the reader can see what was attempted rather
than only what survived.

**VERIFIED, AND STILL A BLOCKER** — the citation stands, and Phase 2 could
still not turn it into a case. It carries to Phase 4 with its citation
intact and emits no case here.

**Do not file a blocker under "Excluded as unspecified."** It is that
section's opposite: the specification is explicit, verified to a page and a
line, and the API has not caught up. Calling such an entry unspecified is a
false statement about the manual, and it buries the one thing worth
reporting. Section 1g already says a divergence between a spec-derived case
and the current code is a finding, not a mistake to smooth over — Phase 4 is
where that finding gets settled.

## Phase 4 — Reconcile the signature

**No blocker, no gate.** A run whose entries all became cases goes straight to
Phase 5 without a prompt. Phase 0 already declines to ask about a mirrored
method name because that would put a question in front of the majority of
runs, and the same reasoning holds here.

With one or more blockers, open the gate **once**, carrying all of them. A
prompt per blocker turns a five-entry method into five interruptions, and it
splits one decision — what shape this method should have — across five
answers that cannot see each other.

Print the blockers in the terminal first, then ask with `AskUserQuestion`. Its
options are short, and a citation, an unwritable line, and a proposed
signature do not fit inside one. The tool carries at most four questions per
call, so more than four blockers ask in batches of four — still not one call
per blocker.

### What each blocker puts in front of the author

- **The verified citation** that demands the case, with its page and line
  span.
- **The line that could not be written** — the `Setup:` or `Expect:` line
  itself, not a summary of it. The author has to see what the current
  signature makes unsayable.
- **A concrete signature**, spelled out as the Rust line it would be in the
  file, not as a description of the change. When it reaches past the method —
  a new variant on a return type, a field on `Screen`, a parameter threaded
  from the CSI dispatcher — name that too, because the author is deciding on
  the whole change rather than on the one line.
- **What each answer costs.** Keeping the current signature means the entry
  emits no case, and the author should read that consequence rather than
  infer it.

Offer two options per blocker — take the proposed signature, or keep the
current one — and let the tool's own free-text answer carry a third signature
the author would rather have. State the cost once and then stop. The decision
is the author's, and a skill that re-argues an answer it was given is worse
than one that never asked.

An answer that never arrives counts as keeping the current signature.

### After the answers

Return to Phase 2 and expand each unblocked entry by its shape, against the
agreed signature.

**The `Source:` rule does not relax here.** An agreed signature makes a
sentence writable; it never makes a case justified. Every case emitted under
one still names the contract entry that demands it, and every `Expect:` line
still ends with its own source in brackets. A case whose only justification
is that the signature was agreed is not a specification-derived case, and the
list stops meaning anything the moment one enters it.

An entry the author left alone emits no case and goes to the report's "left
as is" list with its citation, so the reader sees which specification the API
still does not serve.

**Nothing here edits the repository.** The agreement is a premise for this
enumeration and a record in the report. The author makes the edit.

## Phase 5 — Report

Terminal only. Never save it.

```
# Test cases: Screen::reverse_index
Destination: crates/orzma_vt/src/screen.rs  mod tests::reverse_index
Governs: RI (ESC M)   Specifications: vt510.pdf, ECMA-48.pdf
Enumerated against: the current signature — or, when Phase 4 agreed a
                    change, the agreed signature (N changes), which is
                    not the code as it stands

## Summary          N cases (High n / Medium n / Low n), High first
                    stopping after the High block still covers everything
                    the specification states outright
                    citations verified: N/N
                    blockers: N (agreed n / left as is n)
## Terms tried      every search term and the ladder level it came from,
                    including the ones that reached nothing
## Contract table   with citations
## Test cases       TC-01 … TC-NN
## Excluded as unspecified
   entries with no citation, and entries whose citation failed
   verification (with the failed quote and its match ratio)
## API reconciliation          (omitted when no blocker arose)
   Agreed: each signature the author took, with the blocker it
   resolves and the citation that demanded it
   Left as is: blockers the author kept the current signature for,
   each with its citation and the line that stays unwritable
## Specification conflicts     (omitted when none)
```

A report with no cases uses the same shape — summary, terms tried, and the
sections that have content — and says outright that the manuals govern nothing
this method does. It is an ordinary result, so do not dress it up as an error
or apologise for it. "Terms tried" is what makes it worth reading: it shows the
reader the search that came back empty, which is the one thing distinguishing
this outcome from a lazy run.

There is no review section. Nothing reviewed this list but you, so do not
present it as though something did. Say plainly that the list is
specification-derived and self-verified, and that the author should spot-check
a citation or two before trusting the rest.

When Phase 4 agreed a signature change, say so in the same breath: the cases
below do not compile against the code as it stands. A reader who misses that
transcribes a test for a method that does not exist yet.

## Error handling

| Situation | Behaviour |
| --- | --- |
| The name matches both `Screen` and `interpreter.rs` | Resolve to `Screen` without asking |
| The name is ambiguous within `Screen` itself | `AskUserQuestion` with the candidates |
| The method name resolves to nothing | Stop with an error naming the searched paths |
| The method has no doc comment (`NODOC`) | Descend the ladder to level 2; if the method was expected to carry docs, check for a multi-line attribute first |
| Some terms are absent from `docs/references/` | Drop those, continue, list them under "Terms tried" |
| No term on any level reaches a statement of behaviour | Report 0 cases with every term tried; do not stop with an error |
| A term's hits are all contents, index, or cross-reference lines | Treat it as absent from that manual and descend the precedence order |
| A citation fails verification | Re-record it against the real text or drop the entry; never edit the quote to make it pass. Re-run `verify` on the re-recorded pair before it counts |
| `verify` reports a dropped qualifier | Treat it as a rejection. Narrow the span if the word came from another line or column; never widen the quote to absorb it |
| `verify` reports `SPAN TOO WIDE` | Not a result. Narrow the citation to the lines carrying the statement and run `verify` again |
| The doc-comment extractor prints `NOTFOUND` | Stop and report the file and method tried; never fall back to an unbounded `Read` |
| Phase 2 cannot write a `Setup:` or `Expect:` line for an entry | Record it as a blocker with its kind, citation, and the unwritable line; never drop it, and never emit a partial case |
| A blocker's citation fails verification | It stops being a blocker: it goes under "Excluded as unspecified" and never reaches Phase 4 |
| A citation verifies but the API models no such state | It is an **O** blocker; carry it to Phase 4 with its citation, do not call it unspecified, and emit no case yet |
| The run produced no blocker | Skip Phase 4 entirely and ask nothing |
| The run produced blockers | Phase 4 opens once carrying all of them; never one prompt per blocker |
| The author takes or supplies a signature | Enumerate against it, and say in the report header that the list is not the code as it stands |
| The author keeps the current signature, or answers nothing | Emit no case from that entry; record it under "API reconciliation" as left as is |
| A case can be justified only by an agreed signature | Do not emit it. Phase 4 makes a sentence writable, never a case justified |
| `pdftotext` is unavailable | Stop, suggesting `brew install poppler` |
