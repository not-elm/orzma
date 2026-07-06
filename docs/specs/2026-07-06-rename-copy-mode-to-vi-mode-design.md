# Rename `copy-mode` → `vi-mode`

## Goal

Rename every orzma-side identifier, filename, config key, and doc that names
the **copy-mode concept** to **vi-mode**, so the codebase and user-facing
config use one consistent term. This is a full rename covering both internal
code and user-visible surfaces, with **no back-compat** for existing config.

## Decisions (settled)

1. **Scope:** internal code **and** user-visible surfaces — everything.
2. **Config compat:** hard rename, no aliases. Existing `config.toml` files
   using the old table/action names will error or be ignored. This is
   consistent with the recent hard `ozma`/`ozmux` → `orzma` rebrand.
3. **Naming scheme:** `ViMode*` prefix for all mode-level types, keeping the
   engine-layer alacritty `Vi*` types (`ViMotion`, `ViModeCursor`,
   `SelectionType`) untouched. The `ViMode*` layer sits above the engine's
   `Vi*` layer; the prefix keeps them distinct and collision-free.

## Rename boundary

### Renames — everything naming the copy-mode *concept*

All identifiers containing `CopyMode` / `copy_mode` / `copy-mode` / `copymode`,
**plus** the abbreviated copy-mode config/UI types that carry only a `Copy`
prefix (`CopyMotion`, `CopyScroll`, `CopySelection`, `CopyPrompt*`,
`CopySearchStep`) and the `copy_search` / `copy_gate` / `copy-drag` names.

### Preserved — must NOT change

| Keep | Why |
| --- | --- |
| `Clipboard`, `ClipboardBackend`, `ClipboardPlugin`, `ClipboardStore`, `ClipboardLoad` | Clipboard subsystem — the *copy-to-clipboard* action, a distinct concept from the mode |
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
| `CopySelection` | `ViModeSelection` |
| `CopyPromptDir` | `ViModePromptDir` |
| `CopyPrompt` / `CopyPromptState` / `CopyPromptPlugin` | `ViModePrompt` / `ViModePromptState` / `ViModePromptPlugin` |
| `CopySearchStep` | `ViModeSearchStep` |
| `EnterCopyModeActionEvent` (+ any enter/exit event pair) | `EnterViModeActionEvent` (+ matching) |
| `KeyEffect::CopyMode(..)` | `KeyEffect::ViMode(..)` |
| `ResolvedCopyModeKeys` (`src/action/vi/keymap.rs`) | `ResolvedViModeKeys` |
| `trigger_copy_mode_action` (`src/action/vi/keymap.rs`) | `trigger_vi_mode_action` |
| snake identifiers `copy_mode`, `copy_modes`, `copy_gate`, `copy_search`, `copy_mode_indicator`, `copy_mode_fields`, `copy_action` | `vi_mode`, `vi_modes`, `vi_gate`, `vi_search`, `vi_mode_indicator`, `vi_mode_fields`, `vi_action` |
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
  vi-mode. Existing `config.toml` files using the old names error/are ignored
  — accepted per the hard-rename decision.
- Log / `tracing` / comment strings mentioning "copy-mode" updated to
  "vi-mode" for consistency (e.g. `"copy-mode prompt submit failed"`).

The copy-mode indicator renders `[offset/total]` only (e.g. `[0/429]`) — there
is **no literal "COPY" runtime text** to change.

## Implementation approach

**Guided scripted token-replacement (hybrid), compiler as safety net.**

Ordered, case-sensitive, word-boundary replacements following the naming map
above, plus `git mv` for the seven files, with the Section-"Preserved"
keep-list explicitly protected. A large rename with a small, well-identified
keep-list is exactly where the compiler catches mistakes cheaply: any stray or
wrong replacement fails to compile.

Alternatives considered and rejected:

- **Pure `rust-analyzer` / serena symbol rename** — semantically precise, but
  does not touch string literals (config `"copy-mode"`, `"enter-copy-mode"`,
  docs, log messages, test names) and is slow across ~30 symbols. Kept in mind
  for any symbol whose textual rename is ambiguous.
- **Blind global sed** — fast but risks clobbering the keep-list (clipboard
  `Copy*`, tmux protocol literals). The hybrid guards those explicitly.

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
5. **Grep sweep** — confirm zero remaining `copy_mode` / `CopyMode` /
   `copy-mode` / `copymode` / abbreviated copy-mode `Copy*` tokens **outside
   the documented keep-list** (clipboard, tmux protocol literals, the
   `material.rs` render comment, `copy_command`).

## Out of scope

- Any behavioral change to copy-mode / vi-mode. This is a pure rename.
- Renaming the engine-layer alacritty `Vi*` API or the clipboard subsystem.
- Config back-compat / migration shims.
