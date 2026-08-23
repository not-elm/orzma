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
