---
name: enumerate-test-cases
description: Enumerates the test cases one orzma_vt method owes, deriving every case from a citation in docs/references/ and verifying each citation before emitting it. Settles the list and any signature change with the author, then writes it to docs/todo/ as a markdown working document carrying the Rust to transcribe. Use when the user says "テストケースを洗い出して", "enumerate test cases", "/enumerate-test-cases", or asks which cases a VT method needs before writing its tests.
argument-hint: [Screen::method]
allowed-tools: Read, Grep, Glob, Write, Bash(pdftotext:*), Bash(grep:*), Bash(awk:*), Bash(sed:*), Bash(head:*), Bash(wc:*), Bash(tr:*), Bash(mkdir:*), Bash(python3:*), AskUserQuestion
---

# Enumerate test cases for one method

Enumerate the test cases a method owes, from the VT specification rather than
from its implementation, settle the list with the author, and write it to
`docs/todo/` as a working document the author transcribes tests from.

`Edit` is deliberately absent from `allowed-tools`, and `Write` reaches exactly
one path: `docs/todo/tdd-<file-stem>-<method>.md`, always as a whole-file
replacement. Nothing else in the repository is written — not a source file, not
a test, not another document. `Bash` is granted because `pdftotext` needs it, so
the property is strong rather than airtight; no recipe here writes inside the
repository.

## What this skill will not do

- It does not derive cases from code. Which cases exist, and what each one
  expects, come from the specification and from documented contracts; the
  two-stage rule below bounds what may be read, and when.
- It does not treat an existing test as a source. A test is one author's reading
  of the contract, and seeding the list from it enumerates the tests you already
  have.
- It does not change the method it enumerates. Phase 4 can settle with the
  author that a signature is wrong; the agreement becomes a premise for this
  enumeration and a record in the document, and the edit stays the author's.
- It handles one method per run.

## The two-stage rule

Reading an implementation produces tests that pin whatever the code already
does, including its bugs, and a terminal emulator's bugs read as ordinary code.
That argument bounds the method under enumeration, but it does not stop there:
it holds just as well for the helpers the method delegates to, and for the tests
that already pin the same behaviour. A rule forbidding only the one body lets
the same failure in through the neighbours.

So the run has two stages, and they admit different sources.

**Before the case list is settled** — Phases 0 through 4 — read only the manuals
in `docs/references/`, doc comments and stated invariants through the extractors
in 1a, signatures and the type names, variants, and constructors they mention,
and the names of the test modules together with the file holding them, which is
what the destination rule matches on. No method body: not the one under
enumeration, not the ones it calls, not the bodies of tests that already exist.

**After the author approves the list** — Phase 5, while writing the Rust — read
whatever it takes to turn an approved `Setup:` / `Act:` / `Expect:` line into
code: test helpers, fixture signatures, the neighbouring calls a setup has to
make, and the implementation that decides how many of them a state needs.
Transcription may discover that reaching "the anchor has scrolled out of the
viewport" takes three `line_feed()` calls. It may not discover a fourth case, a
new expectation, or a reason to drop one.

**The list is closed at approval.** A case or an `Expect:` line that first occurs
to you during transcription does not enter the document. Carry it to the author
as a remark instead, and let them decide whether it earns another run.

## Output language

Test names, the `Asserts that …` line, `Case:` paragraphs, and the Rust the
document carries are written in English, because they land in the repository as
`///` doc comments and this repository requires in-code comments to be English.
Everything the document says in its own voice — headings, the reasoning
paragraph under each case, the appendix, and the terminal narration — follows
the language of the conversation, which is what the existing notes in
`docs/todo/` already do.

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
do not govern produces a run with no cases rather than a refusal. A
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

Record every term tried and the level it came from, and carry both to the
appendix's terms-tried list. No grep verdict stands behind a refusal any more, so that
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
individually and listed in the appendix's terms-tried list. When no term on any level reaches
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
for that reason in the appendix's unspecified list, so the reader
knows what the enumeration does not cover rather than mistaking the list for
complete.

**No body reaches this stage**, per the two-stage rule. Read the signature and
the types it mentions (`DamageSpan`, `EraseLineMode`, `GridSize`) — the output has
to name real constructors and real variants, and that is API shape, not
behaviour. A divergence between a spec-derived case and the current code is a
finding, not a mistake to smooth over.

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

The rest return `Option<DamageSpan>`, which is orzma's own contract; no VT
manual mentions it. Derive it from the spec-described state change:

| Spec-described change | Expected return |
| --- | --- |
| A bounded contiguous span of rows changes contents, and the span is inside the viewport | `Some(DamageSpan::rows(first, last))` |
| Content moves across the whole screen — a scroll, `ED 2`, a reset | `Some(DamageSpan::Full)` |
| Rows changed contents, but all of them sit outside the viewport | `None` |
| Nothing on screen changes — cursor position, pen, deferred-wrap flag | `None` |

`DamageSpan` has exactly two variants: `Full` and `Rows { first, last }`
(`crates/orzma_vt/src/frame/damage.rs:24`). Sending every content change
to `Full` gives a wrong expectation for `erase_in_line` and for both partial
`erase_in_display` modes, which return `DamageSpan::rows(..)` and are already
pinned by four existing tests. There is no metadata variant: a change no
viewport row shows reports nothing, and the emit layer's own diffs decide
whether a frame is still owed.

The specification stays the source for *what changes*; this table maps that to
*what is reported*.

A return type outside those two shapes takes neither branch.
`ScrollRegion::scroll_span` returns `RangeInclusive<ScreenLine>`, and no
`DamageSpan` reaches its caller at all. Derive the expected value from the
specification-described state directly and have the `Expect:` line name the
concrete value — the inclusive range from the top margin to the bottom margin,
for that method. The Damage table does not apply, and neither does the
`Expect:` source tag that cites it.

### Source tags

Every `Expect:` line ends with a tag naming what demands it, and there are
exactly three kinds. A line that can name none of the three is dropped, the same
way an uncited case is.

| Kind | Tag | What it names |
| --- | --- | --- |
| 1 | `C1`, `C2`, … | A contract-table entry: a manual statement, verified in Phase 3 |
| 2 | a short mnemonic you assign (`LC`, `CAP`, `PI`) | A doc comment or a stated invariant in this repository |
| 3 | `Damage` | The Damage decision table above — the table, not the type — available only to a method returning `Option<DamageSpan>` |

Kinds 1 and 3 are already pinned — one by Phase 3, the other by a fixed table in
this file. Kind 2 is the loose one, so it carries its own ledger.

**The kind-2 ledger.** A repository source is admitted only as a full entry, and
the tag is unusable until that entry exists:

```
LC — crates/orzma_vt/src/placement.rs:178, PlacementStore::evict_lost_anchors
     "<the sentence, quoted from the doc comment>"
     justifies: `evicted` names the id of every placement dropped
```

Naming the item is not enough. An item name asserts that a contract lives
somewhere inside it; the quote is what shows the contract says *this*. Without
the quote, the tag records a hunch about a doc comment, and the reader has no
more to check than the reader of an untagged line.

Two document classes are **not** admissible as kind 2, for the same reason this
skill does not seed lookups from them. An existing test is one author's reading
of the contract. A `// NOTE:` inside an implementation is reached only by reading
that body, which the two-stage rule forbids at this stage. Notes under
`docs/todo/` are not admissible either: they are proposals, and Phase 5 searches
them for conflicts rather than citing them.

**A kind-2 source contradicting a verified manual statement produces no case.**
Report it as a specification conflict, with both quotes, the way 1e already
handles two manuals disagreeing. The repository does not get to overrule the
manual quietly.

### Cases with no manual source

A case whose `Expect:` lines are all kind 2 has no manual behind it at all.
Number it `TC-A1`, `TC-A2`, … rather than `TC-01`, keep it in the same list, and
say in one sentence why it is there. `PlacementStore::reset` earns one: the most
natural implementation, `*self = Self::new()`, rewinds the id counter and breaks
the invariant `InstanceId` states, and no manual has an opinion about that.

The separate numbering is the whole mechanism. The list stays complete, and a
reader can still tell at a glance which rows the specification demands from the
ones the repository's own contracts demand.

### Priority

- **High** — behaviour the spec states outright, and the boundaries of
  conditions it states outright.
- **Medium** — behaviour that follows from the spec but depends on a default
  or a mode.
- **Low** — combinations of modes the spec does not address individually.

Emit cases in that order and say so in the document, so an author who stops
after the High block still has every case the specification states outright.

### Obvious cases

Phase 4 puts the case list in front of the author, and a list where every row
needs a decision is a list nobody reads to the end. A case is **obvious**, and
so is settled without asking, when all three of these hold:

1. Its contract entry's `Shape` is unconditional.
2. Its `Setup:` uses nothing outside this closed list — the type's constructor
   at any size, placing the cursor, and filling rows with distinguishable
   characters.
3. Every `Expect:` line is tagged either with the single contract entry the case
   came from, or with `Damage` for the return line that entry implies.

Condition 3 admits the `Damage` line deliberately. That line follows from the
same entry through the fixed table above with no judgement in between, so
counting it as a second source would make nearly every unconditional case on an
`Option<DamageSpan>` method non-obvious and empty the category. One kind-2 tag, or a
second contract entry, is enough to make a case non-obvious — both carry
judgement the author should see. A `TC-A` case is non-obvious by construction.

Anything failing one of the three goes to the gate, and so does anything you are
unsure about. A case wrongly sent to the gate costs the author one line of
reading; a case wrongly settled without asking is the one they never see.

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

### The per-case block

Each case becomes one section of the document, in this shape. The example is in
Japanese because that was the conversation's language; the English parts are the
ones that land in the repository.

````
## TC-03 — 上マージン上の RI は画面を下へスクロールする

| | |
| - | - |
| Setup | `Screen::new(GridSize { cols: 80, rows: 24 }, 0)`; `margins.top = ScreenLine(2)`; カーソルを `ScreenLine(2)` へ; 各行に識別可能な文字を置く |
| Act | `screen.reverse_index()` |
| Expect | マージン領域が1行下へスクロールする **[C2]** ／ カーソルは `ScreenLine(2)` に留まる **[C1]** ／ 上マージン行が erase セルになる **[C2]** ／ deferred wrap が解除される **[LC]** ／ `Some(DamageSpan::Full)` を返す **[Damage]** |

（なぜこの setup がその状態に届くのか。どの実装を防ぐのか。参照する `file.rs:行`。）

```rust
/// Asserts that a reverse index on the top margin scrolls the margin
/// region down rather than moving the cursor off it.
///
/// Case: the top margin sits on the third row and the cursor is on it
/// when RI arrives.
#[test]
fn a_reverse_index_at_the_top_margin_scrolls_the_screen_down() { … }
```
````

The heading states the case in the conversation's language; the test name below
it is the English the author transcribes. The reasoning paragraph is the
document's own voice, and it carries what the tables cannot: why this setup is
the one that reaches the state, and which plausible implementation the case
exists to catch.

Every `Expect:` line ends with its tag, and that rule binds **per line, not per
case.** Binding it to the case instead would let an untagged assertion ride
inside a case whose header cites a real sentence, and the reader has no way to
tell it apart from the tagged lines around it. That is the shape an
implementation-derived expectation takes when it enters a specification-derived
list: not a fabricated citation, but a well-cited case carrying one extra line
nobody asked the manual about. Tagging each line makes the untagged assertion
visible instead of inferred.

The `///` block holds exactly two parts — the `Asserts that …` line and a
`Case:` paragraph — because `.claude/rules/rust.md` requires that and the author
transcribes it verbatim. `Case:` carries the scenario and nothing else: no
restatement of the assertions, no policy, no speculation about how a broken
implementation would misbehave. The reasoning paragraph above the code is where
that material belongs, and it stays in the document rather than entering the
repository. Any policy line the rule also requires is the author's to write,
because it states a decision the specification does not make.

The Rust is written only after Phase 4 approves the case, under the second half
of the two-stage rule, and it is never promised to compile — see Phase 5.

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

### What a blocker has to carry

A blocker stands on a chain, and the chain has to close end to end:

```
a verified contract entry
  → the Setup: or Expect: line it demands, which the current signature
    cannot express
  → the kind that failure takes (R, P, O, or T)
  → the smallest signature change that makes the line writable
  → the case IDs that become writable under that change
```

Every link is checkable, and a proposal missing one is not a blocker.

The first link is the load-bearing one: **the entry demanding the change is the
same entry the change serves.** A verified citation from elsewhere in the run
satisfies nothing — it makes a proposal look sourced without making it sourced,
which is worse than an obviously unsourced one.

The last link is not evidence, it is scope: it tells the author what the answer
buys. A proposal that enables no case buys nothing and is not asked.

**A signature you would rather have is not a blocker.** "Writable but clumsy",
"this reaches private state over and over", "this parameter wants to be a type"
— each may be right, and none is a specification failing to be served. Record
them under "Unsourced suggestions" in the appendix, where the author reads them
as remarks. They never reach the gate, and they never change a case.

That partition is the guard. The gate opens on your judgement that the signature
is wrong, which is wider than a citation that will not fit; without the partition
that width is the one hole through which unsourced design walks into the list —
an opinion arriving in the same frame as a citation, and settled by the same
answer.

Reaching private state is not itself a defect, and it is worth saying so
explicitly, because it looks like one. The destination rule below has a `Setup:`
line touch `margins.top` directly, on purpose: the tests live in the same file as
the type.

A blocker goes to Phase 3 before it goes anywhere else. Phase 4 settles it on the
strength of its chain and on nothing else, so a blocker whose citation fails
verification was never a blocker.

## Phase 3 — Verify every citation

Verify **every** citation the run recorded, not a sample — manual citations and
kind-2 ledger entries alike, the ones behind cases you are about to emit and the
ones behind blockers alike. This is the only check standing between a
plausible-sounding sentence and a case the author will trust, and a fabricated
citation reads exactly like a real one — that is what makes it dangerous. You
are verifying your own work, which is precisely the situation where skipping the
check feels safest and is least safe.

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

### Verifying a kind-2 ledger entry

A repository quote needs no `verify`. There is no table geometry to reassemble
and no page index to compute, so the sentence either appears verbatim or does
not:

```bash
grep -n -F -- "<the quoted sentence>" <file>
```

A doc comment wraps across `///` lines, so match one line's worth at a time, or
strip the `///` prefixes into a single line first and match against that. Zero
hits means the sentence is not there. Correct the quote against the real text or
drop the ledger entry — never adjust the quote until it matches, which is the
same rubber stamp `REJECTED` warns about below.

Count kind-2 entries in `citations verified: N/N` alongside the manual ones. A
document that verifies its manual quotes and takes its repository quotes on trust
reports a number meaning less than it looks.

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
nothing about the new pair, and an entry that reaches the document on the
strength of a run against text it no longer carries is exactly the unchecked
citation this phase exists to catch. Re-run, and count it in `citations
verified: N/N` only once it passes.

A dropped entry goes to the appendix's unspecified list with the quote that
failed and its match ratio, so the reader can see what was attempted rather
than only what survived.

**VERIFIED, AND STILL A BLOCKER** — the citation stands, and Phase 2 could
still not turn it into a case. It carries to Phase 4 with its citation
intact and emits no case here.

**Do not file a blocker in the unspecified list.** It is that
section's opposite: the specification is explicit, verified to a page and a
line, and the API has not caught up. Calling such an entry unspecified is a
false statement about the manual, and it buries the one thing worth
reporting. Section 1g already says a divergence between a spec-derived case
and the current code is a finding, not a mistake to smooth over — Phase 4 is
where that finding gets settled.

## Phase 4 — Settle the list with the author

One gate, opened once, carrying everything that needs an answer: the non-obvious
cases and the signature proposals together. A second gate for the signature would
split one decision — what shape this method has, and which cases it therefore
owes — across two answers that cannot see each other. A gate per blocker would
turn a five-entry method into five interruptions.

The gate always opens, even with no blocker and no non-obvious case, because the
list itself is what is being settled. A run where everything came out obvious
asks one question and gets one answer.

### What the terminal shows first

`AskUserQuestion`'s options are short, and a case block, a citation, and a
proposed signature do not fit inside one. Print all of it first, then ask.

1. **The obvious cases, by name only**, under one line saying how many were
   settled without asking. No question follows them. They stay visible, so a
   misclassification is something the author can object to rather than something
   the run hides.
2. **Every non-obvious case in full** — the Setup/Act/Expect table, the tags, and
   the reasoning paragraph. Not the Rust; that is written after approval, under
   the second half of the two-stage rule.
3. **Every signature proposal**, each carrying its whole chain: the verified
   entry, the line the current signature cannot express, the kind, and the Rust
   signature line as it would appear in the file — not a description of the
   change. When it reaches past the method, a new variant on a return type, a
   field on `Screen`, a parameter threaded from the CSI dispatcher, name that
   too: the author is deciding on the whole change, not on the one line.

A proposal is shown with **both versions of every case it affects** — the case as
it stands under the current signature, unwritable and marked so, and the complete
`Setup:` / `Act:` / `Expect:` it becomes under the proposal. Naming the case IDs
alone would have the author approve cases they have not read, and those cases
would then reach the document having passed no gate at all.

### The questions

Two at most, well inside `AskUserQuestion`'s cap of four per call:

- **The case list** — approve, revise (free text), or cancel.
- **The signature proposals**, when there are any — take them, or keep the
  current signature. The tool's own free-text answer carries a third signature
  the author would rather have.

Signature proposals are answered **as one change-set**, not one question each.
Four proposals would otherwise exhaust the call, and they would split a single
decision about the method's shape across four answers.

State each answer's cost once, then stop. Keeping the current signature means the
affected entries emit no case, and the author should read that consequence rather
than infer it. The decision is the author's, and a skill that re-argues an answer
it was given is worse than one that never asked.

### The three terminal states

**approve** — the list is closed. Phase 5 writes the Rust and the document.

**revise** — the author names cases by ID and says what changes. Apply the
changes, then **re-check what the change touched**: an edited `Expect:` line
needs its tag re-derived, and re-verified when the tag is kind 1 or kind 2; an
edited `Setup:` line can move a case across the obvious boundary in either
direction. Re-present the affected cases only — reprinting an approved list
buries the diff — and ask again. The list is closed only on an `approve`.

**cancel, or no answer at all** — nothing is written. Print the list in the
terminal and stop. A run the author walked away from is not an approved run, and
this skill's one write is not the place to guess.

### After a signature is taken

Return to Phase 2 and expand each unblocked entry by its shape against the agreed
signature.

**The tag rule does not relax here.** An agreed signature makes a sentence
writable; it never makes a case justified. Every case emitted under one still
names the contract entry that demands it, and every `Expect:` line still ends
with its own tag. A case whose only justification is that the signature was
agreed is not a specification-derived case, and the list stops meaning anything
the moment one enters it.

If that expansion yields a case the author has not seen — because the shape
produced more than the proposal predicted — that is a revise, not an approval.
Present it and ask again.

An entry the author left alone emits no case and goes to the appendix's "left as
is" list with its citation, so the reader sees which specification the API still
does not serve.

**Nothing here edits the repository.** The agreement is a premise for this
enumeration and a record in the document. The author makes the edit.

## Phase 5 — Write the document

### Where it goes

`docs/todo/tdd-<file-stem>-<method>.md`, where `<file-stem>` is the defining
file's name without its extension: `screen.rs` gives `tdd-screen-reset.md`,
`screen/grid.rs` gives `tdd-grid-reset.md`, `placement.rs` gives
`tdd-placement-reset.md`.

The stem, not the type. `orzma_vt` holds five methods named `reset` —
`Screen::reset`, `Grid::reset`, `TabStops::reset`,
`CharacterSetMapping::reset`, and `PlacementStore::reset` — each in its own
file, so the stem separates all five while the bare method name collides five
ways. A stem collision is still possible in principle, when one file defines two
types sharing a method name, and the header check below is what catches it.

**Read the path first, and check the header.** The document's first line is
`# Test cases: <Type>::<method>`, so it names its own target and the check is one
line long. A document whose header names this same target is a re-run: overwrite
it whole, without asking. The author is not
consulted about that, and `docs/` is tracked in git, so a bad run is recoverable
with `git checkout`. A document whose header names a **different** target is a
filename collision, not a re-run: **do not write.** Stop, print the list in the
terminal, and report both targets. That check is not a confirmation prompt; it is
the one case where the path does not mean what it appears to.

`Write` reaches this path and nothing else, always as a whole-file replacement.

### Before writing: conflicts with the existing notes

`docs/todo/` already holds hand-written design notes, and they propose APIs.
Search it for the target type, the method, and each control function the run
touched, and list what disagrees under "Conflicts with docs/todo" in the
appendix. `ris.md` proposes `pub(crate) fn clear(&mut self) -> Vec<InstanceId>`
for dropping every placement, while `tdd-placement-reset.md` is written against
`pub(crate) fn reset(&mut self) -> Vec<InstanceId>` — one operation under two
names, which is exactly what nobody notices unless something looks.

Report the conflict; do not resolve it, and do not adopt the note's version. A
note under `docs/todo/` is a proposal, which is why it is not admissible as a
kind-2 source either.

### The shape of the document

```
# Test cases: Screen::reverse_index

（リード文: 対象、参照した仕様、検証 N/N、Phase 4 の結果。合意した signature が
あるなら、以下が現状の木に対するものではないこと。）

## テストケース一覧

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-01 | `a_…` | C1 | High |
| TC-A1 | `a_…` | PI | High |

Source タグ — **C1**: vt510.pdf p.64 L2435（…）／**LC**: …／**PI**: …

（どこまでが仕様由来で、どれがそうでないかの1〜2行。）

テストコードは <file> の <mod tests::…> へ追加する。既存の <…> ヘルパを使う。
<signature> を前提にしているので、現状の木に対してコンパイル{できる|できない}。

## TC-01 — …
（Setup/Act/Expect の表、理由の段落、Rust）
…

## 付録
### 契約表                （全エントリ、citation 付き）
### 探索した語
### 仕様にないもの
### 仕様の矛盾            （無ければ省略）
### API 改定              （無ければ省略）
### 出典なしの改善提案    （無ければ省略）
### docs/todo との衝突    （無ければ省略）
```

Cases go High, then Medium, then Low, and the document says so, so an author who
stops after the High rows still has every case the specification states outright.
All cases sit in one list, `TC-A` rows included; the numbering is what separates
them, not a section break.

**The appendix carries the full contract table**, not the compressed legend near
the top. That legend is a reading aid, ten entries do not fit in four lines, and
a `C7` the reader cannot trace back to a page, a line span, and a quote is a tag
standing in for a citation. "探索した語" carries every search term and the ladder
level it came from, including the ones that reached nothing — that record is the
only thing making "searched and found nothing" falsifiable. "仕様にないもの"
carries entries with no citation and entries whose citation failed verification,
each with the failed quote and its match ratio. "API 改定" carries each signature
the author took, with the blocker it resolves and the citation that demanded it,
and each blocker left as is, with the line that stays unwritable.

### What the header must say

Two claims, and both are load-bearing:

- **Whether the Rust compiles against the tree as it stands.** It does not when
  Phase 4 agreed a signature, and it does not when the method is still a stub. A
  reader who misses that transcribes a test for a method that does not exist yet.
- **That the Rust is unbuilt.** This skill does not compile what it writes, and
  in the agreed-signature case it could not: there is nothing to compile against.
  Present it as a proposal to transcribe, not as something ready to paste and
  run.

There is no review section. Nothing reviewed this list but you, so do not present
it as though something did. Say plainly that the list is specification-derived
and self-verified, and that the author should spot-check a citation or two before
trusting the rest.

### When there are no cases

Write nothing. A document holding zero cases is a todo that is not a todo, and
`docs/todo/` is a directory the author reads. Report in the terminal instead —
the count, and every term tried, which is what distinguishes this outcome from a
lazy run. It is an ordinary result, so do not dress it up as an error or
apologise for it.

If a document already exists at the path, say so and leave it alone. It is now
stale, and the author should hear that from this run rather than discover it
later.

### The terminal, after the write

The path, the counts by priority, the names of the cases settled as obvious, and
`citations verified: N/N`. The document carries everything else.

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
| A method body would answer the question, before the list is settled | Do not read it — the method's own, a helper's, or an existing test's. The two-stage rule admits bodies only while transcribing an approved case |
| A case or an `Expect:` line first occurs to you while transcribing | The list is closed. Do not add it; carry it to the author as a remark |
| A kind-2 tag with no ledger entry behind it | Unusable. Drop that `Expect:` line, exactly as an untagged one |
| A kind-2 quote `grep -F` does not find | Correct the quote against the real text or drop the entry; never adjust the quote until it matches |
| A kind-2 source contradicts a verified manual statement | Emit no case. Report it under specification conflicts with both quotes |
| Every tag on a case is kind 2 | Number it `TC-A<n>`, keep it in the same list, and say in one sentence why it is there |
| A case meets all three obvious conditions | Settle it without asking; name it in the terminal under the count settled that way |
| Phase 2 cannot write a `Setup:` or `Expect:` line for an entry | Record it as a blocker with its kind, citation, and the unwritable line; never drop it, and never emit a partial case |
| A blocker's citation fails verification | It stops being a blocker: it goes under the appendix's unspecified list and never reaches Phase 4 |
| A citation verifies but the API models no such state | It is an **O** blocker; carry it to Phase 4 with its citation, do not call it unspecified, and emit no case yet |
| A signature proposal whose chain does not close end to end | Not a blocker. It goes under "Unsourced suggestions" in the appendix, never reaches the gate, and never changes a case |
| Several signature proposals in one run | One change-set, one question — never one question per proposal |
| The gate has no blocker and no non-obvious case | It still opens. The list itself is what is being settled; one question, one answer |
| The author answers `revise` | Apply the edits, re-derive and re-verify the tags they touched, re-check obviousness, re-present the affected cases only, and ask again |
| The author answers `cancel`, or does not answer | Write nothing. Print the list in the terminal and stop |
| The author takes or supplies a signature | Enumerate against it, and say in the document header that the list is not the code as it stands |
| The author keeps the current signature | Emit no case from that entry; record it under "API 改定" as left as is |
| A case can be justified only by an agreed signature | Do not emit it. Phase 4 makes a sentence writable, never a case justified |
| Expanding against an agreed signature yields a case the author has not seen | That is a revise, not an approval. Present it and ask again |
| The destination document exists and its header names this target | Overwrite it whole, without asking. `docs/` is tracked in git |
| The destination document exists and its header names a different target | Do not write. Stop, print the list in the terminal, and report both targets |
| The run produced no case | Write nothing. Report in the terminal, and say so if a stale document sits at the path |
| A note under `docs/todo/` disagrees with the run | Report it under the appendix's conflicts; do not resolve it, and do not adopt the note's version |
| `pdftotext` is unavailable | Stop, suggesting `brew install poppler` |
