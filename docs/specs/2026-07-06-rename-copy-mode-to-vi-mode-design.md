# Rename `copy-mode` → `vi-mode`

## Goal

Rename every orzma-side identifier, filename, config key, and doc that names
the **copy-mode concept** to **vi-mode**, so the codebase and user-facing
config use one consistent term. This is a full rename covering both internal
code and user-visible surfaces, with **no back-compat** for existing config.

## Decisions (settled)

1. **Scope:** internal code **and** user-visible surfaces — everything.
2. **Config compat:** hard rename, no aliases. Existing `config.toml` files
   using the old names **fail at startup with a config error** — both
   `OrzmaConfigs` (`lib.rs:30`) and `[shortcuts]` (`shortcuts.rs:328`) carry
   `#[serde(deny_unknown_fields)]`, so a stale `[copy-mode]` table or
   `enter-copy-mode` action is rejected (mapped to `ParseToml`, `lib.rs:84`),
   not silently ignored. This is consistent with the recent hard `ozma`/`ozmux`
   → `orzma` rebrand.
3. **Naming scheme:** `ViMode*` prefix for all mode-level types, keeping the
   engine-layer alacritty `Vi*` types (`ViMotion`, `ViModeCursor`,
   `SelectionType`) untouched. The `ViMode*` layer sits above the engine's
   `Vi*` layer; the prefix keeps them distinct and collision-free.

## Rename boundary

### Renames — everything naming the copy-mode *concept*

**The authoritative rule is substring-level:** every `CopyMode` → `ViMode` and
`copy_mode` → `vi_mode` / `copy-mode` → `vi-mode` / `copymode` → `vimode`
substring is rewritten wherever it appears. This catches identifiers the map
below does not spell out — e.g. `DuplicateCopyModeKey`
(`crates/orzma_configs/src/copy_mode.rs:312`), `DuplicateCopyModeKeys`
(`error.rs:45`), `parse_copy_mode_key` (`copy_mode.rs:125`),
`format_copy_mode_dupes` (`error.rs:88`) → `DuplicateViModeKey`,
`DuplicateViModeKeys`, `parse_vi_mode_key`, `format_vi_mode_dupes`. The naming
map (next section) is **illustrative of the non-obvious and collision cases**,
not an exhaustive whitelist.

Additionally rename the abbreviated copy-mode config/UI identifiers that carry
only a `Copy` prefix (`CopyMotion`, `CopyScroll`, `CopyPromptDir`,
`CopyPrompt*`, `CopySearchStep`, snake `copy_prompt`, `resolved_copy`,
`copy_search`, `copy_gate`, `copy-drag`) — and `CopySelection`, **but only the
`orzma_configs` config enum** (see the ambiguity note in the keep-list).

Because several `Copy*` names are **ambiguous** (some are clipboard-copy, not
the mode), the ambiguous ones (`CopySelection`) MUST be renamed with a
symbol-aware tool (rust-analyzer / serena `rename_symbol`) scoped to the
`orzma_configs::copy_mode::CopySelection` type — NOT a blind token replace.

### Preserved — must NOT change

| Keep | Why |
| --- | --- |
| `Clipboard`, `ClipboardBackend`, `ClipboardPlugin`, `ClipboardStore`, `ClipboardLoad` | Clipboard subsystem — the *copy-to-clipboard* action, a distinct concept from the mode |
| `TmuxMouseEffect::CopySelection` (`src/input/mouse/button/tmux/effect.rs:45`) — a **second** `CopySelection`, distinct from the config enum | Clipboard-copy mouse effect: its apply arm triggers `TerminalSelectionCopy` (`apply.rs:127`). A blind `CopySelection → ViModeSelection` would corrupt it — hence the symbol-scoped rename above |
| `TerminalSelectionCopy` event, `MouseEffect::Copy` variant | Clipboard-copy events (`src/action/terminal/selection.rs:46` writes the selection to the clipboard) — the mode's abbreviated `Copy*` sweep will catch these; they are kept |
| tmux `-X` protocol literals: `"search-forward"`, `"search-backward"`, `"jump-forward"`, `"jump-backward"`, `"jump-to-forward"`, `"jump-to-backward"`, `send-keys -X` | tmux's own wire protocol — changing them breaks tmux control-mode |
| Engine alacritty types: `ViMotion`, `ViModeCursor`, `SelectionType`, `TerminalHandle::enter_vi_mode` / `exit_vi_mode`, `Term::vi_mode_cursor`, `toggle_vi_mode` | alacritty's own API, already correctly named; the `ViMode*` concept layer sits above it |
| `copy-paste` comment in `crates/orzma_tty_renderer/src/material.rs` | Unrelated rendering comment ("copy-paste that samples the wrong texture") |
| `PromptKind::copy_command()` fn name (`crates/orzma_tmux/src/command/copymode.rs` → renamed file, kept fn) | Its sole job is to return tmux's copy-command protocol strings; keep the name with a note. (Renaming to `vi_command` was offered and declined by default.) |

## Naming map (`ViMode*` scheme)

| Current | New |
| --- | --- |
| `CopyModeState` | `ViModeState` |
| `CopyModePlugin` | `ViModePlugin` |
| `CopyModeIndicator` / `CopyModeIndicatorPlugin` | `ViModeIndicator` / `ViModeIndicatorPlugin` |
| `CopyModeKeymapPlugin` | `ViModeKeymapPlugin` |
| `CopyModeGate` | `ViModeGate` |
| `CopyModeAction` | `ViModeAction` |
| `CopyModeMessage` | `ViModeMessage` |
| `CopyModeKey` / `CopyModeBaseKey` / `CopyModeNamedKey` | `ViModeKey` / `ViModeBaseKey` / `ViModeNamedKey` |
| `CopyModeConfig` | `ViModeConfig` |
| `CopyModeKeyParseError` | `ViModeKeyParseError` |
| `CopyMotion` | `ViModeMotion` (engine `ViMotion` stays) |
| `CopyScroll` | `ViModeScroll` |
| `CopySelection` **(only `orzma_configs::copy_mode::CopySelection` — symbol-scoped rename)** | `ViModeSelection` |
| `CopyPromptDir` | `ViModePromptDir` |
| `CopyPrompt` / `CopyPromptState` / `CopyPromptPlugin` | `ViModePrompt` / `ViModePromptState` / `ViModePromptPlugin` |
| `CopySearchStep` | `ViModeSearchStep` |
| `EnterCopyModeActionEvent` (+ any enter/exit event pair) | `EnterViModeActionEvent` (+ matching) |
| `KeyEffect::CopyMode(..)` | `KeyEffect::ViMode(..)` |
| `ResolvedCopyModeKeys` (`src/action/vi/keymap.rs`) | `ResolvedViModeKeys` |
| `trigger_copy_mode_action` (`src/action/vi/keymap.rs`) | `trigger_vi_mode_action` |
| snake identifiers `copy_mode`, `copy_modes`, `copy_gate`, `copy_search`, `copy_mode_indicator`, `copy_mode_fields`, `copy_action` | `vi_mode`, `vi_modes`, `vi_gate`, `vi_search`, `vi_mode_indicator`, `vi_mode_fields`, `vi_action` |
| abbreviated snake locals/fields `copy_prompt` (`src/ui/copy_search.rs:170`), `resolved_copy` (`src/input/keyboard/handler.rs:70`) | `vi_mode_prompt`, `resolved_vi_mode` |
| kebab identifiers `copy-search`, `copy-drag` | `vi-search`, `vi-drag` |
| test fn names containing `copy_mode` / `copy_key` (e.g. `copy_mode_rebind_and_unbind`, `copy_key_shadowed_by_gui`) | `vi_mode_*` / `vi_key_*` equivalents |

**`src/action/vi/` module path stays `vi`** — it already carries no "copy" in
its path and is the correct vi-action namespace. Its already-`Vi*` event types
(`ViMotionRequest`, `ViScrollRequest`, `ViPromptRequest`) keep their names;
only their doc comments switch "copy-mode" → "vi-mode".

## File / module renames (`git mv` + `mod` declarations)

Per the repo's no-`mod.rs` rule, each rename updates the declaring `mod` line
in the parent file.

| From | To |
| --- | --- |
| `src/ui/copy_mode.rs` | `src/ui/vi_mode.rs` |
| `src/ui/copy_mode_indicator.rs` | `src/ui/vi_mode_indicator.rs` |
| `src/ui/copy_search.rs` | `src/ui/vi_search.rs` |
| `crates/orzma_configs/src/copy_mode.rs` | `crates/orzma_configs/src/vi_mode.rs` |
| `crates/orzma_configs/tests/fixtures/copy_mode_binding.toml` | `crates/orzma_configs/tests/fixtures/vi_mode_binding.toml` |
| `crates/orzma_configs/tests/fixtures/duplicate_copy_mode_key.toml` | `crates/orzma_configs/tests/fixtures/duplicate_vi_mode_key.toml` |
| `crates/orzma_tmux/src/command/copymode.rs` | `crates/orzma_tmux/src/command/vi_mode.rs` |

## User-facing changes (breaking — no back-compat)

- Config table `[copy-mode]` → `[vi-mode]`: update the
  `#[serde(rename = "copy-mode")]` attribute to `"vi-mode"` and the struct
  field `copy_mode` → `vi_mode` in `crates/orzma_configs/src/lib.rs`.
- Shortcut action `enter-copy-mode` → `enter-vi-mode` (config action name and
  all its test/reference occurrences).
- `docs/configs.md` and any other docs referencing copy-mode updated to
  vi-mode. Existing `config.toml` files using the old names fail at startup
  (see Decisions §2) — accepted per the hard-rename decision.
- **GitHub issue templates** — the dropdown option `Copy mode` in
  `.github/ISSUE_TEMPLATE/bug_report.yml:48` and
  `.github/ISSUE_TEMPLATE/feature_request.yml:40` → `Vi mode` (spaced,
  user-visible text — not caught by an underscore/hyphen sweep).
- Log / `tracing` / comment strings mentioning "copy-mode" updated to
  "vi-mode" for consistency (e.g. `"copy-mode prompt submit failed"`).

The copy-mode indicator renders `[offset/total]` only (e.g. `[0/429]`) — there
is **no literal "COPY" runtime text** to change.

## Implementation approach

**Guided token-replacement for unambiguous substrings + symbol-aware rename
for ambiguous names, compiler as safety net.**

- **Unambiguous substrings** (`CopyMode`→`ViMode`, `copy_mode`→`vi_mode`,
  `copy-mode`→`vi-mode`, `copymode`→`vimode`): ordered, case-sensitive
  token replacement — these cannot collide with clipboard names.
- **Ambiguous `Copy*` names** — chiefly `CopySelection`, which exists as
  **both** the `orzma_configs` mode enum (rename) and
  `TmuxMouseEffect::CopySelection` (clipboard, keep): use a symbol-aware tool
  (rust-analyzer / serena `rename_symbol`) scoped to the exact
  `orzma_configs::copy_mode::CopySelection` type, so the mouse-effect variant
  is untouched. Same care for any other abbreviated `Copy*` name that a grep
  shows is used in more than one concept.
- `git mv` for the seven files, then update declaring `mod` lines.
- The Section-"Preserved" keep-list is explicitly protected throughout.

A large rename with a small, well-identified keep-list is exactly where the
compiler catches mistakes cheaply: any stray or wrong replacement fails to
compile.

Alternatives considered and rejected:

- **Pure `rust-analyzer` / serena symbol rename for everything** —
  semantically precise, but does not touch string literals (config
  `"copy-mode"`, `"enter-copy-mode"`, docs, log messages, test names, the
  `.github` YAML options) and is slow across ~30 symbols. Reserved for the
  ambiguous symbols only.
- **Blind global sed for everything** — fast but clobbers the keep-list
  (`TmuxMouseEffect::CopySelection`, `TerminalSelectionCopy`, `Clipboard*`,
  tmux protocol literals). The split approach guards those explicitly.

### Commit grouping

Logical, each compiling independently where practical:

1. `orzma_configs` — config table `[vi-mode]`, action `enter-vi-mode`, the
   `vi_mode.rs` enums, fixtures, tests.
2. Engine / tmux crates — `orzma_tmux` command module rename; any
   `orzma_tty_engine` / `orzma_tty_renderer` comment updates.
3. `src/` — input, action, ui, render wiring.
4. `docs/` — `configs.md` and design/plan notes.

## Verification

1. `cargo build` — the primary net; the rename must compile with zero stray
   references.
2. `cargo test` — full workspace, with attention to `orzma_configs`:
   - the `[vi-mode]` table parse test (formerly `copy_mode_binding.toml`),
   - the duplicate-key test (formerly `duplicate_copy_mode_key.toml`),
   - the `enter-vi-mode` shortcut resolution/serialization tests.
3. `cargo clippy --workspace` + `cargo fmt` (or `just fix-lint`).
4. `pnpm -r test` — SDK is unaffected, confirm green.
5. **Grep sweep** — case-insensitive over `copy[_ -]?mode` (covers `copy_mode`,
   `copy-mode`, `copymode`, **and spaced `copy mode` / `Copy mode`** — the last
   catches the `.github` YAML options), plus the abbreviated copy-mode `Copy*`
   config/UI names. Confirm zero remaining hits **outside the documented
   keep-list**. Expected survivors that are NOT violations (grep must exclude
   these explicitly, else they read as failures):
   - clipboard: `Clipboard*`, `TerminalSelectionCopy`, `MouseEffect::Copy`,
     `TmuxMouseEffect::CopySelection`,
   - tmux protocol literals (`search-forward` …) and `PromptKind::copy_command`,
   - the `material.rs` "copy-paste" render comment.
   Also grep the `.github/` and `docs/` trees, not just `src` / `crates`.

## Out of scope

- Any behavioral change to copy-mode / vi-mode. This is a pure rename.
- Renaming the engine-layer alacritty `Vi*` API or the clipboard subsystem.
- Config back-compat / migration shims.
