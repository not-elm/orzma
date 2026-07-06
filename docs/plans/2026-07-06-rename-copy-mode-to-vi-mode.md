# copy-mode → vi-mode Rename Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename every orzma-side identifier, filename, config key, and doc that names the *copy-mode concept* to *vi-mode*, with no config back-compat, leaving behavior unchanged.

**Architecture:** This is a pure mechanical rename verified by the compiler and the existing test suite — **not** a behavior change, so the normal "write a failing test first" TDD loop does not apply. Each task instead follows: apply rename (git mv + scoped substring `sed`, or symbol-aware rename for ambiguous names) → `cargo build` → `cargo test` → scoped grep → commit. Tasks are ordered so the workspace compiles green at every commit because each task fully renames its cluster of symbols across the whole tree at once.

**Tech Stack:** Rust (edition 2024, toolchain 1.95, Cargo workspace), TypeScript (pnpm, unaffected), `sed` (BSD/macOS), `ripgrep` (`rg`), `git mv`, and (for the one ambiguous symbol) serena `rename_symbol` / rust-analyzer.

**Spec:** `docs/specs/2026-07-06-rename-copy-mode-to-vi-mode-design.md`

## Global Constraints

- **Naming scheme:** mode-level types become `ViMode*`; the engine-layer alacritty `Vi*` types (`ViMotion`, `ViModeCursor`, `SelectionType`, `TerminalHandle::enter_vi_mode`/`exit_vi_mode`, `Term::vi_mode_cursor`, `toggle_vi_mode`) are **never** renamed.
- **Keep-list — never rename these** (they are clipboard-copy or tmux wire-protocol, not the mode):
  - `Clipboard`, `ClipboardBackend`, `ClipboardPlugin`, `ClipboardStore`, `ClipboardLoad`
  - The `TmuxMouseEffect` mouse click-drag-to-copy gesture family — `BeginCopyDrag`, `ExtendCopyDrag`, `CopySelection` (`src/input/mouse/button/tmux/effect.rs:28-46`) — and the `copy-drag` **comments** describing them. This is the "copy to clipboard via mouse drag" interaction, not the vi-mode keyboard concept; keeping the whole enum family consistent with the already-kept `CopySelection`. **(Boundary call — flag on review; overridable if the drag family should also become `ViMode*`.)**
  - `TerminalSelectionCopy` event, `MouseEffect::Copy` variant, `on_terminal_selection_copy` observer, and clipboard-assertion test names like `release_after_bare_click_does_not_copy` / `release_from_unbegun_selecting_does_not_copy` (they assert *no clipboard copy* happened)
  - `inline_anchor_is_copy_and_eq` (a `Copy`-trait test, unrelated) and all `#[derive(Copy, …)]` / `.copy_from_slice` / `.copy()` uses
  - tmux `-X` protocol literals: `"search-forward"`, `"search-backward"`, `"jump-forward"`, `"jump-backward"`, `"jump-to-forward"`, `"jump-to-backward"`, and `send-keys -X`
  - `PromptKind::copy_command()` fn name and the `<copy-command>` placeholder in its doc comment (`crates/orzma_tmux/src/command/copymode.rs:43`) — the file it lives in IS renamed, but these name tmux's `-X` protocol command, so they stay
  - `copy-paste` comment in `crates/orzma_tty_renderer/src/material.rs`
  - In **docs prose**: tmux's own feature names `copy-mode`, `copy-mode-vi`, `mode-keys`, and "tmux's own copy-mode key tables" — these describe real tmux features and stay.
  - In **code comments**: the one external-tool equivalence reference `crates/orzma_tty_renderer/src/schema/cursor.rs:12` ("= tmux copy mode") — names tmux's actual feature; stays.
- **No back-compat:** old config names must fail at startup (both `OrzmaConfigs` and `[shortcuts]` carry `#[serde(deny_unknown_fields)]`). Do not add aliases.
- **Scope of automated `sed`:** code tasks operate on `src` and `crates` ONLY (never `docs/`, `target/`, `node_modules/`, `.git/`). Docs and `.github` are a separate, hand-edited task.
- **Comment language:** English only (repo rule). Any comment you touch stays English.
- **Rust rules:** no `mod.rs`; imports at top; the repo's clippy/fmt gate must pass. This rename does not add items, so visibility/ordering rules are preserved as-is.
- **macOS `sed` in-place:** use `sed -i '' 's/…/…/g'`. Select files with `rg -l 'PATTERN' src crates`.

---

### Task 1: Core `copy_mode` / `CopyMode` / `copy-mode` rename + file moves + config table/action

Renames the bulk of the mode concept across the whole workspace in one coordinated pass, including the three `copy_mode*.rs` file moves and the two config fixtures, and flips the user-facing config table `[copy-mode]` → `[vi-mode]` and action `enter-copy-mode` → `enter-vi-mode`. These three substrings (`CopyMode`, `copy_mode`, `copy-mode`) cannot collide with any keep-list name, so a plain substring replace is safe.

**Files:**
- Rename (git mv): `src/ui/copy_mode.rs` → `src/ui/vi_mode.rs`
- Rename (git mv): `src/ui/copy_mode_indicator.rs` → `src/ui/vi_mode_indicator.rs`
- Rename (git mv): `crates/orzma_configs/src/copy_mode.rs` → `crates/orzma_configs/src/vi_mode.rs`
- Rename (git mv): `crates/orzma_configs/tests/fixtures/copy_mode_binding.toml` → `crates/orzma_configs/tests/fixtures/vi_mode_binding.toml`
- Rename (git mv): `crates/orzma_configs/tests/fixtures/duplicate_copy_mode_key.toml` → `crates/orzma_configs/tests/fixtures/duplicate_vi_mode_key.toml`
- Modify (via sed, whole tree): every file under `src` and `crates` containing `copy_mode` / `CopyMode` / `copy-mode`. Notable hand-verified spots: `src/ui.rs:7-9` (`pub mod copy_mode;` etc.), `src/main.rs:43-44,86-88` (plugin imports/registration), `crates/orzma_configs/src/lib.rs:13,17,35,36,108-109`, `crates/orzma_configs/tests/load.rs:107,123,125`.

**Interfaces:**
- Produces (later tasks and consumers rely on these new names): `ViModeConfig`, `ViModeAction`, `ViModeState`, `ViModePlugin`, `ViModeIndicator`, `ViModeIndicatorPlugin`, `ViModeKeymapPlugin`, `ViModeGate`, `ViModeMessage`, `ViModeKey`, `ViModeBaseKey`, `ViModeNamedKey`, `ViModeKeyParseError`, `ResolvedViModeKeys`, `trigger_vi_mode_action`, `EnterViModeActionEvent`, `KeyEffect::ViMode(..)`, `DuplicateViModeKey(s)`, `parse_vi_mode_key`, `format_vi_mode_dupes`; the `OrzmaConfigs.vi_mode: ViModeConfig` field; config table `[vi-mode]`; action `enter-vi-mode`; modules `crate::ui::vi_mode`, `crate::ui::vi_mode_indicator`, `orzma_configs::vi_mode`.
- Consumes: nothing from other tasks (first task).

- [ ] **Step 1: Confirm clean tree and record the baseline reference count**

```bash
cd /Users/taiga/workspace/ozmux/wt/rename-copy-mode
git status --short   # expect clean
rg -c --stats -e 'copy[_-]?mode' -e 'CopyMode' src crates | tail -1   # baseline for later comparison
```

Expected: working tree clean; a nonzero baseline count printed.

- [ ] **Step 2: git mv the five files**

```bash
git mv src/ui/copy_mode.rs src/ui/vi_mode.rs
git mv src/ui/copy_mode_indicator.rs src/ui/vi_mode_indicator.rs
git mv crates/orzma_configs/src/copy_mode.rs crates/orzma_configs/src/vi_mode.rs
git mv crates/orzma_configs/tests/fixtures/copy_mode_binding.toml crates/orzma_configs/tests/fixtures/vi_mode_binding.toml
git mv crates/orzma_configs/tests/fixtures/duplicate_copy_mode_key.toml crates/orzma_configs/tests/fixtures/duplicate_vi_mode_key.toml
```

Expected: five renames staged; `cargo build` would now FAIL (mod lines still say `copy_mode`) — that is fixed in Step 3.

- [ ] **Step 3: Apply the three substring replacements across `src` and `crates`**

```bash
# CamelCase types: CopyModeAction -> ViModeAction, DuplicateCopyModeKeys -> DuplicateViModeKeys, etc.
rg -l 'CopyMode' src crates | xargs sed -i '' 's/CopyMode/ViMode/g'
# snake identifiers, module names, fields, fn names, fixture path strings, test fn names
rg -l 'copy_mode' src crates | xargs sed -i '' 's/copy_mode/vi_mode/g'
# kebab strings: config table rename value, "enter-copy-mode" action, "[copy-mode]" error text
rg -l 'copy-mode' src crates | xargs sed -i '' 's/copy-mode/vi-mode/g'
```

Expected: no output (sed is silent). This rewrites `#[serde(rename = "copy-mode")]` → `"vi-mode"`, the `enter_copy_mode`/`"enter-copy-mode"` action to `enter_vi_mode`/`"enter-vi-mode"`, `pub mod copy_mode;` → `pub mod vi_mode;`, the `OrzmaConfigs.copy_mode` field → `vi_mode`, the fixture path strings in `tests/load.rs`, and the serialized-JSON expected string in `shortcuts.rs`.

- [ ] **Step 4: Review orzma_tmux comments for tmux's own copy-mode (rare keep-case)**

```bash
rg -n 'vi-mode|vi_mode' crates/orzma_tmux/src | rg -i 'tmux'   # inspect any comment now saying "tmux vi-mode"
```

Expected: If a **comment** now reads "tmux vi-mode" but actually describes tmux's *own* copy-mode command/behavior (not orzma's feature), revert that one comment's word back to "copy-mode" by hand. Identifier and string changes stay. (In practice orzma_tmux comments describe orzma's own flow; usually nothing to revert.)

- [ ] **Step 5: Build the workspace**

Run: `cargo build`
Expected: PASS (compiles clean). If it fails on an unresolved `copy_mode`/`CopyMode` path, a file move or a ref was missed — re-run the Step 3 greps to find the straggler.

- [ ] **Step 6: Run the config crate tests, then the whole suite**

Run: `cargo test -p orzma_configs`
Expected: PASS — including the `[vi-mode]` fixture parse test and `duplicate_vi_mode_key_rejected`.

Run: `cargo test`
Expected: PASS (whole workspace).

- [ ] **Step 7: Scoped grep — no stray core tokens remain in code**

```bash
rg -n -e 'copy_mode' -e 'CopyMode' -e 'copy-mode' src crates
```

Expected: no output. (Abbreviated `Copy*` names like `CopyMotion` and the `copymode` module are handled in Tasks 2–3 and are expected to still appear — they do NOT match these three patterns.)

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: rename copy_mode/CopyMode/copy-mode -> vi_mode/ViMode/vi-mode

Config table [copy-mode] -> [vi-mode], action enter-copy-mode -> enter-vi-mode
(hard rename, no back-compat). Moves the three copy_mode*.rs files and the two
config fixtures. Behavior unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Rename the `orzma_tmux` `copymode` module → `vi_mode`

The tmux command module is spelled `copymode` (no separator), so Task 1's `copy_mode` replacement did not touch it. It has exactly one declaration site and one re-export.

**Files:**
- Rename (git mv): `crates/orzma_tmux/src/command/copymode.rs` → `crates/orzma_tmux/src/command/vi_mode.rs`
- Modify: `crates/orzma_tmux/src/command.rs:4` (`mod copymode;`) and `:12` (`pub use copymode::{Prompt, PromptKind};`)

**Interfaces:**
- Consumes: nothing from Task 1 (independent crate concern).
- Produces: module path `orzma_tmux::command::vi_mode` (the re-exports `Prompt`, `PromptKind` keep their names, so downstream `orzma_tmux::{Prompt, PromptKind}` is unchanged). `PromptKind::copy_command()` fn name is intentionally kept (returns tmux protocol strings).

- [ ] **Step 1: git mv the module file**

```bash
git mv crates/orzma_tmux/src/command/copymode.rs crates/orzma_tmux/src/command/vi_mode.rs
```

Expected: rename staged; build would FAIL until Step 2.

- [ ] **Step 2: Update the module declaration and re-export**

```bash
sed -i '' 's/copymode/vi_mode/g' crates/orzma_tmux/src/command.rs
```

Expected: `crates/orzma_tmux/src/command.rs:4` becomes `mod vi_mode;` and `:12` becomes `pub use vi_mode::{Prompt, PromptKind};`. Verify the `PromptKind::copy_command` fn name inside `vi_mode.rs` was NOT changed (there is no `copymode` substring inside it, so it is untouched):

```bash
rg -n 'fn copy_command' crates/orzma_tmux/src/command/vi_mode.rs   # expect the fn still present
```

- [ ] **Step 3: Build**

Run: `cargo build -p orzma_tmux`
Expected: PASS.

- [ ] **Step 4: Confirm no `copymode` token remains**

```bash
rg -n 'copymode' src crates
```

Expected: no output.

- [ ] **Step 5: Test and commit**

Run: `cargo test -p orzma_tmux`
Expected: PASS.

```bash
git add -A
git commit -m "refactor(orzma_tmux): rename command::copymode module -> vi_mode

Keeps PromptKind::copy_command (returns tmux -X protocol strings).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Rename abbreviated `Copy*` config/UI types (`CopyMotion`, `CopyScroll`, `CopyPrompt*`, `CopySearchStep`) + `CopySelection` (symbol-scoped)

These CamelCase types carry only a `Copy` prefix. Four are unambiguous and safe to substring-replace. `CopySelection` is **ambiguous** — a second `CopySelection` exists as `TmuxMouseEffect::CopySelection` (clipboard, keep) — so it MUST be renamed with a symbol-aware tool scoped to the `orzma_configs` config enum only.

**Files:**
- Modify (via sed, whole tree): every `src`/`crates` file containing `CopyMotion`, `CopyScroll`, `CopyPromptDir`, `CopyPrompt`, `CopyPromptState`, `CopyPromptPlugin`, `CopySearchStep`. Definitions live in `crates/orzma_configs/src/vi_mode.rs` (the ex-`copy_mode.rs`), `src/ui/vi_search.rs` (the `CopyPrompt*` types), `src/action/vi/keymap.rs`.
- Modify (symbol-scoped rename): `orzma_configs::copy_mode::CopySelection` (now in `crates/orzma_configs/src/vi_mode.rs:255`, enum `CopySelection`) and its references in `src/action/vi/keymap.rs:14,229-233,285,296` and `crates/orzma_configs/src/vi_mode.rs:298,416,418,420`.

**Interfaces:**
- Consumes from Task 1: definitions now live in `crates/orzma_configs/src/vi_mode.rs` and use `ViModeAction`.
- Produces: `ViModeMotion`, `ViModeScroll`, `ViModePromptDir`, `ViModePrompt`, `ViModePromptState`, `ViModePromptPlugin`, `ViModeSearchStep`, `ViModeSelection`. Note `ViModeMotion` is distinct from the engine's `ViMotion` (kept). `ViModeSelection` maps to the engine's `SelectionType` via `fn selection_type(selection: ViModeSelection) -> SelectionType`.

- [ ] **Step 1: Substring-replace the four unambiguous types (order-safe: `CopyPrompt` covers all its suffixes)**

```bash
rg -l 'CopyPrompt'     src crates | xargs sed -i '' 's/CopyPrompt/ViModePrompt/g'
rg -l 'CopyMotion'     src crates | xargs sed -i '' 's/CopyMotion/ViModeMotion/g'
rg -l 'CopyScroll'     src crates | xargs sed -i '' 's/CopyScroll/ViModeScroll/g'
rg -l 'CopySearchStep' src crates | xargs sed -i '' 's/CopySearchStep/ViModeSearchStep/g'
```

Expected: silent. `CopyPromptDir`/`CopyPromptState`/`CopyPromptPlugin` all become `ViModePrompt*` via the first line.

- [ ] **Step 2: Symbol-scoped rename of the config `CopySelection` enum (NOT sed)**

Use serena `rename_symbol` (or rust-analyzer "Rename Symbol") on the enum defined at `crates/orzma_configs/src/vi_mode.rs` — `pub enum CopySelection` — renaming it to `ViModeSelection`. This updates exactly its references (`src/action/vi/keymap.rs`, `crates/orzma_configs/src/vi_mode.rs`) and leaves `TmuxMouseEffect::CopySelection` untouched.

If a symbol-aware tool is unavailable, do it by hand: rename `pub enum CopySelection` → `pub enum ViModeSelection` and update only the references that resolve to the `orzma_configs` type (the `selection_type` fn param + arms in `keymap.rs:229-233`, the `CopyModeAction::Selection(CopySelection::…)` sites now spelled `ViModeAction::Selection(CopySelection::…)` in `keymap.rs:285,296` and `vi_mode.rs:298,416,418,420`, and the `use` at `keymap.rs:14`). Do NOT touch `src/input/mouse/button/tmux/*`.

- [ ] **Step 3: Verify the mouse-effect `CopySelection` survived untouched**

```bash
rg -n 'CopySelection' src crates
```

Expected: the ONLY remaining `CopySelection` hits are in `src/input/mouse/button/tmux/effect.rs`, `apply.rs`, `decide.rs` (the `TmuxMouseEffect::CopySelection` clipboard variant). No `CopySelection` in `orzma_configs` or `src/action/vi/` remains.

- [ ] **Step 4: Build and test**

Run: `cargo build`
Expected: PASS.

Run: `cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: rename abbreviated copy-mode types to ViMode* (CopyMotion/CopyScroll/CopyPrompt*/CopySearchStep/CopySelection)

CopySelection renamed only for the orzma_configs config enum; the
TmuxMouseEffect::CopySelection clipboard variant is intentionally kept.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Rename remaining snake/kebab abbreviations (`copy_search`, `copy_prompt`, `resolved_copy`, `copy_gate`, `copy_action`, `copy_key`, `_active_copy_pane`, `copy-search`)

The last mode-concept identifiers that carry a bare `copy` (no `mode`) and so were not caught by Task 1. Includes the `copy_search.rs` file move. **Note:** `copy-drag` is deliberately NOT renamed — those comments describe the kept `TmuxMouseEffect` copy-drag gesture family (see Global Constraints keep-list). `copy-search` IS renamed — those comments describe the copy-mode search prompt modal (`CopyPrompt`→`ViModePrompt`, now `vi_search`).

**Files:**
- Rename (git mv): `src/ui/copy_search.rs` → `src/ui/vi_search.rs`
- Modify: `src/ui.rs:9` (`pub(crate) mod copy_search;`), `src/main.rs:44` (`copy_search::…` import).
- Modify (via sed, whole tree): files under `src`/`crates` containing `copy_search`, `copy_prompt`, `resolved_copy`, `copy_gate`, `copy_action`, `copy_key`, `_active_copy_pane`, `copy-search`. Notable: `src/ui/vi_search.rs` (the ex-`copy_search.rs`, `copy_prompt` locals), `src/input/keyboard/handler.rs:70,153` + `key_effect.rs:88` (`resolved_copy`), `src/input/keyboard/key_effect.rs:163` (`copy_action` local) and `:811` (`copy_key_shadowed_by_gui` test fn), `src/input/tmux/gate.rs` (`copy_gate`), `src/input/mouse/wheel/tmux.rs:517` (`_active_copy_pane` test local).

**Interfaces:**
- Consumes from Task 1: the `ViMode*` types these locals/fields are typed against (e.g. `resolved_copy: Res<ResolvedViModeKeys>` → `resolved_vi_mode`).
- Produces: module `crate::ui::vi_search`; locals/fields `vi_mode_prompt`, `resolved_vi_mode`, `vi_gate`, `vi_action`.

- [ ] **Step 1: git mv the search UI file**

```bash
git mv src/ui/copy_search.rs src/ui/vi_search.rs
```

Expected: rename staged; build FAILS until Step 2.

- [ ] **Step 2: Apply the snake/kebab replacements across `src` and `crates`**

```bash
rg -l 'copy_search'      src crates | xargs sed -i '' 's/copy_search/vi_search/g'
rg -l 'copy_prompt'      src crates | xargs sed -i '' 's/copy_prompt/vi_mode_prompt/g'
rg -l 'resolved_copy'    src crates | xargs sed -i '' 's/resolved_copy/resolved_vi_mode/g'
rg -l 'copy_gate'        src crates | xargs sed -i '' 's/copy_gate/vi_gate/g'
rg -l 'copy_action'      src crates | xargs sed -i '' 's/copy_action/vi_action/g'
rg -l 'copy_key'         src crates | xargs sed -i '' 's/copy_key/vi_key/g'
rg -l '_active_copy_pane' src crates | xargs sed -i '' 's/_active_copy_pane/_active_vi_pane/g'
rg -l 'copy-search'      src crates | xargs sed -i '' 's/copy-search/vi-search/g'
```

Expected: silent. `pub(crate) mod copy_search;` → `mod vi_search;` and the `copy_search::` import in `main.rs` are fixed here. `copy-drag` comments are intentionally left untouched (kept mouse-gesture family).

- [ ] **Step 3: Build and test**

Run: `cargo build`
Expected: PASS.

Run: `cargo test`
Expected: PASS.

- [ ] **Step 4: Full code-side grep sweep — zero mode-concept `copy` tokens outside the keep-list**

```bash
rg -n -i 'copy[_ -]?mode' src crates
rg -n -e 'CopyMotion' -e 'CopyScroll' -e 'CopyPrompt' -e 'CopySearchStep' \
      -e 'copy_search' -e 'copy_prompt' -e 'resolved_copy' -e 'copy_gate' -e 'copy_action' \
      -e 'copy_key' -e '_active_copy_pane' -e 'copy-search' -e 'copymode' src crates
```

Expected: no output from the SECOND command. The FIRST command (`copy[_ -]?mode`) also matches spaced **"copy mode"** in code comments/test-strings — those are handled by Task 4B, not here, so the first command will still show ~68 comment hits at this point. Confirm (via `git stash` if unsure) that Task 4 did not *add* any; the pre-existing spaced-comment set is Task 4B's job.

Note the deliberate NON-matches (these are the keep-list and must NOT be searched for / must NOT be renamed): `copy-drag` comments, `TmuxMouseEffect::{BeginCopyDrag, ExtendCopyDrag, CopySelection}`, `Clipboard*`, `TerminalSelectionCopy`, `MouseEffect::Copy`, `on_terminal_selection_copy`, `*_does_not_copy` test names, `copy_command`, the tmux `-X` literals, and the `material.rs` `copy-paste` comment. None of them match the patterns above, so a clean sweep is expected.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: rename remaining copy-mode snake/kebab identifiers to vi_* (copy_search/copy_prompt/resolved_copy/copy_gate/copy_action/copy-drag)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4B: Rename spaced "copy mode" in code comments and test strings

Tasks 1–4 renamed only `copy_mode` / `copy-mode` / `CopyMode` (and the abbreviated `Copy*` types). None targeted the **spaced** "copy mode" that appears in ~68 code comments, doc-comments (`///`, `//!`), and test-assertion string literals across `src` and `crates`. These overwhelmingly describe orzma's *own* feature and must become "vi mode" for consistency with the renamed identifiers. None are user-visible runtime strings (verified: only comments and `#[cfg(test)]` assertion messages).

**Files:** every `src`/`crates` file containing spaced "copy mode" / "Copy mode" — chiefly `src/render/tmux.rs`, `src/input/keyboard/key_effect.rs`, `src/ui/vi_mode.rs`, `src/input/default_mode.rs`, `crates/orzma_configs/src/vi_mode.rs`, and ~20 others.

**Keep (do NOT rename) — the one external-tool equivalence reference:** `crates/orzma_tty_renderer/src/schema/cursor.rs:12` — `/// When the user is in alacritty vi mode (= tmux copy mode), the server` — "tmux copy mode" names tmux's *actual* feature for the reader; renaming it to "tmux vi mode" would be factually wrong (tmux's mode is `copy-mode`).

**Interfaces:** none (comments/strings only, zero behavior change, no new tests).

- [ ] **Step 1: Protect the one keep, then sed the rest**

```bash
# Temporarily mark the external-tool reference so the sed skips it, then restore.
sed -i '' 's/= tmux copy mode/= tmux KEEPCOPY mode/' crates/orzma_tty_renderer/src/schema/cursor.rs
rg -l 'copy mode' src crates | xargs sed -i '' 's/copy mode/vi mode/g'
rg -l 'Copy mode' src crates | xargs sed -i '' 's/Copy mode/Vi mode/g'
sed -i '' 's/= tmux KEEPCOPY mode/= tmux copy mode/' crates/orzma_tty_renderer/src/schema/cursor.rs
```

Expected: silent. (If `rg -l 'Copy mode'` matches nothing, `xargs` runs `sed` with no files and prints a usage error — harmless; or guard with `rg -l 'Copy mode' src crates | xargs -r sed …` where supported. On macOS without `-r`, skip the line if the prior `rg` shows no capital-C hits.)

- [ ] **Step 2: Verify the keep survived and the rest are renamed**

```bash
rg -n -i 'copy mode' src crates
```

Expected: exactly ONE line — `crates/orzma_tty_renderer/src/schema/cursor.rs:12` (`= tmux copy mode`). No other "copy mode" remains.

- [ ] **Step 3: Build, test, format**

Run: `cargo build` → PASS (comments/strings only; cannot break compilation, but test-assertion *string* edits are inside test code, so build must still pass).
Run: `cargo test` → PASS (renamed assertion messages still assert the same conditions).
Run: `cargo fmt` (comment-length changes may shift rustfmt's wrapping of doc-comments).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor: rename spaced 'copy mode' -> 'vi mode' in code comments and test strings

Keeps the one external-tool equivalence reference (cursor.rs: '= tmux copy mode').

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Update user-facing docs and GitHub issue templates

Docs are not compiled, so this is last. `docs/configs.md` mixes orzma's own feature (rename) with tmux's own setting names (keep), so it is hand-edited, not sed'd. Historical `docs/plans/` and `docs/specs/` are left as records (except our own new spec, which intentionally uses both terms).

**Files:**
- Modify (hand-edit): `docs/configs.md` — orzma's own references only.
- Modify: `.github/ISSUE_TEMPLATE/bug_report.yml:48` and `.github/ISSUE_TEMPLATE/feature_request.yml:40` (`Copy mode` dropdown option → `Vi mode`).

**Interfaces:** none (documentation only).

- [ ] **Step 1: Edit `docs/configs.md` — rename orzma's feature, KEEP tmux's setting names**

Change these (orzma's own config surface):
- The `[copy-mode]` table heading (line ~139) → `[vi-mode]`.
- `enter-copy-mode` in the action list/table (lines ~102, ~235, ~299, ~310) → `enter-vi-mode`.
- The "Copy-mode keys" section title and body references to *orzma's* copy-mode/`[copy-mode]` table → "Vi-mode keys" / `[vi-mode]`.
- Comments like "Alacritty vi mode in Default, tmux copy-mode under tmux" — rewrite orzma's feature name to vi-mode while leaving the description of tmux's underlying mode accurate.

KEEP verbatim (tmux's own features):
- Line ~285: "your `tmux.conf` copy-mode / copy-mode-vi customizations and the tmux `mode-keys` option have no effect" — `copy-mode`, `copy-mode-vi`, `mode-keys` stay.
- Line ~283: "no longer follow tmux's own copy-mode key tables at all" — stays.
- Line ~417: stock `copy-mode-vi` keys — stays.

After editing, sanity-check that every remaining `copy-mode` in the file is a deliberate reference to *tmux's* feature:

```bash
rg -n 'copy-mode|copy mode|Copy mode' docs/configs.md
```

Expected: only lines that describe tmux's own `copy-mode`/`copy-mode-vi`/`mode-keys` survive.

- [ ] **Step 2: Edit the GitHub issue templates**

```bash
sed -i '' 's/Copy mode/Vi mode/g' .github/ISSUE_TEMPLATE/bug_report.yml .github/ISSUE_TEMPLATE/feature_request.yml
rg -n 'Copy mode|Vi mode' .github/ISSUE_TEMPLATE/
```

Expected: both files now show `Vi mode`; no `Copy mode` remains.

- [ ] **Step 3: Commit**

```bash
git add docs/configs.md .github/ISSUE_TEMPLATE/bug_report.yml .github/ISSUE_TEMPLATE/feature_request.yml
git commit -m "docs: rename orzma copy-mode -> vi-mode in configs.md and issue templates

Keeps references to tmux's own copy-mode / copy-mode-vi / mode-keys features.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Final verification (lint, format, full test, SDK, grep sweep)

A single gate confirming the whole rename is coherent and the repo's quality bar passes.

**Files:** none created; this task only runs checks and, if needed, a formatting commit.

**Interfaces:** none.

- [ ] **Step 1: Lint + format**

Run: `cargo clippy --workspace --all-targets` then `cargo fmt --check`
Expected: clippy clean (no new warnings from the rename); `fmt --check` clean. If `fmt --check` reports diffs, run `cargo fmt` and include the result in Step 4's commit.

- [ ] **Step 2: Full Rust test suite**

Run: `cargo test`
Expected: PASS across the workspace.

- [ ] **Step 3: TypeScript suite (unaffected, confirm green)**

Run: `pnpm -r test`
Expected: PASS (the SDK does not reference copy-mode; this confirms no accidental change).

- [ ] **Step 4: Final repo-wide grep sweep (excluding historical docs and keep-list)**

```bash
# Code + live docs + .github must be clean of mode-concept copy tokens:
rg -n -i 'copy[_ -]?mode' src crates .github docs/configs.md
rg -n -e 'CopyMotion' -e 'CopyScroll' -e 'CopyPrompt' -e 'CopySearch' -e 'copymode' src crates
```

Expected: The first command returns ONLY the documented keeps: the deliberate tmux-feature references in `docs/configs.md` (from Task 5 Step 1) AND the single code-comment keep `crates/orzma_tty_renderer/src/schema/cursor.rs:12` ("= tmux copy mode", from Task 4B). The second returns nothing. Everything else must be empty. Historical `docs/plans/` and `docs/specs/` (except the new design spec) are intentionally NOT swept — they are records of past work.

If Step 1 required a `cargo fmt` run, commit it:

```bash
git add -A
git commit -m "style: cargo fmt after copy-mode -> vi-mode rename

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 5: Smoke-run (optional, if a display is available)**

Run: `cargo run` — enter vi-mode (default `Cmd+S` / `enter-vi-mode`), confirm the indicator chip shows `[offset/total]` and cursor/scroll/selection work; under an adopted `tmux -CC` session confirm copy-mode selection still works. This exercises the renamed action end-to-end.
Expected: vi-mode behaves exactly as copy-mode did before the rename.
