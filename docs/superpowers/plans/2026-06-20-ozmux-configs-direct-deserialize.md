# OzmuxConfigs Direct Deserialize Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate `RawConfigs` and all `*Patch` structs in `crates/ozmux_configs`, making `OzmuxConfigs` and each section's resolved type derive `Deserialize` directly.

**Architecture:** Migrate one section at a time *inside* the existing `RawConfigs` (flip each `Option<SectionPatch>` field to `Option<SectionConfig>` with full-replace merge, relying on container `#[serde(default)]` for per-field defaults), keeping the crate compiling and green after every task. Once every section is direct-deser, collapse `RawConfigs` into `OzmuxConfigs` itself and delete `raw.rs`. This works because `RawConfigs` already mixes both styles today (`shortcuts: Option<Shortcuts>` and `startup_mode: Option<StartupMode>` are already resolved types with full-replace).

**Tech Stack:** Rust 2024 (toolchain 1.95), `serde` derive, `toml`. No new dependencies.

## Global Constraints

- Behavior-preserving for parse results and errors EXCEPT two deliberate breaking changes: (1) `[font]` flat schema `normal = "<path>"` (old nested `[font.normal] path=` becomes a load error); (2) deprecated tmux `auto_connect` acceptance removed (a config carrying it now errors via `deny_unknown_fields`).
- `deny_unknown_fields` preserved per current section: present on top-level (`OzmuxConfigs`), `keyboard`, `ozma`, `tmux`, `shortcuts`/`Bindings`; absent on `theme`, `mouse`, `osc_webview`, `inactive_pane`, `font`.
- The serde rule that makes this work: container-level `#[serde(default)]` fills each *missing* field from the type's `Default` impl while keeping fields present in the TOML. Precedent: `Bindings` (`shortcuts.rs`).
- Rust rules (`.claude/rules/rust.md`): no `mod.rs`; comments only `// TODO:`/`// NOTE:`/`// SAFETY:`; `//!` on every module file; all `use` at top in one contiguous block; doc comments on every `pub` item; private items last in a block; mutable params before immutable; visibility minimized (anything used only in its module is private).
- All comments in English.
- Run the section crate's tests with `cargo test -p ozmux_configs`. Lint with `cargo clippy -p ozmux_configs` and format with `cargo fmt -p ozmux_configs`. Build the consumer with `cargo check --workspace`.
- Every commit message ends with the trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Work on the current branch `config-font` (already a feature branch; do not commit to `main`).

## File Structure

- `crates/ozmux_configs/src/lib.rs` — owns `OzmuxConfigs`, the load pipeline (`load`/`load_with_env`), and (after Task 2) the private `normalize()`/`validate()` methods. Final home of the migrated integration tests.
- `crates/ozmux_configs/src/raw.rs` — shrinks each task (patch fields flip to resolved types) and is DELETED in the final task.
- `crates/ozmux_configs/src/{theme,mouse,keyboard,osc_webview,ozma,tmux,inactive_pane,font}.rs` — each loses its `*Patch` struct and gains direct-deser attributes on its resolved type; tests rewritten to deserialize the resolved type directly.
- `crates/ozmux_configs/src/{shortcuts,startup}.rs` — unchanged (already direct-deser); only referenced by the collapse task.
- `src/font.rs` (consumer) — field renames `font.normal_path` → `font.normal` etc. and test-fixture schema updates, in the font task only.

---

### Task 1: Commit the family/style-removal baseline

**Files:**
- Modify: `crates/ozmux_configs/src/font.rs` (already modified in the working tree — uncommitted)

**Interfaces:**
- Consumes: nothing.
- Produces: a clean working tree so subsequent tasks start from a committed baseline.

- [ ] **Step 1: Confirm the working tree contains only the font.rs family/style removal**

Run: `git status --short`
Expected: exactly one line — ` M crates/ozmux_configs/src/font.rs`

- [ ] **Step 2: Confirm the crate is green at this baseline**

Run: `cargo test -p ozmux_configs 2>&1 | grep "test result:"`
Expected: `test result: ok. 120 passed; 0 failed; ...` (plus the doc-test line `0 passed`)

- [ ] **Step 3: Commit the baseline**

```bash
git add crates/ozmux_configs/src/font.rs
git commit -m "refactor(configs): drop FontConfig family fields and FacePatch family/style

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Move `validate` to `OzmuxConfigs::validate`

**Files:**
- Modify: `crates/ozmux_configs/src/lib.rs` (add method, update `parse_and_validate`)
- Modify: `crates/ozmux_configs/src/raw.rs` (remove `pub(crate) fn validate` and its tests if any)

**Interfaces:**
- Consumes: `OzmuxConfigsError::{DuplicateChords, InvalidFontSize}` (existing, `error.rs`), `Bindings::validate_no_conflicts` (existing, `shortcuts.rs`).
- Produces: `OzmuxConfigs::validate(&self) -> OzmuxConfigsResult<()>` (private; callers in `lib.rs` only).

- [ ] **Step 1: Add the failing test (lib.rs test module)**

In `crates/ozmux_configs/src/lib.rs`, inside `#[cfg(test)] mod mouse_integration_tests` (or a new `#[cfg(test)] mod validate_tests`), add:

```rust
#[test]
fn validate_rejects_font_size_out_of_range() {
    let mut configs = OzmuxConfigs::default();
    configs.font.size = 0.0;
    assert!(configs.validate().is_err(), "size 0.0 must fail validation");
    configs.font.size = 11.25;
    assert!(configs.validate().is_ok(), "in-range size validates");
}
```

- [ ] **Step 2: Run it to verify it fails to compile (method missing)**

Run: `cargo test -p ozmux_configs validate_rejects_font_size_out_of_range 2>&1 | tail -5`
Expected: compile error `no method named validate`

- [ ] **Step 3: Add the method and update the call site**

In `crates/ozmux_configs/src/lib.rs`, add to `impl OzmuxConfigs` (place after `parse_and_validate`, before any private helper):

```rust
    fn validate(&self) -> OzmuxConfigsResult<()> {
        if let Err(dupes) = self.shortcuts.bindings.validate_no_conflicts() {
            return Err(OzmuxConfigsError::DuplicateChords(dupes));
        }
        let size = self.font.size;
        if !(size > 0.0 && size <= 200.0) {
            return Err(OzmuxConfigsError::InvalidFontSize { size });
        }
        Ok(())
    }
```

Change `parse_and_validate` to call the method instead of `raw::validate`:

```rust
    fn parse_and_validate(text: &str, path: &Path) -> OzmuxConfigsResult<Self> {
        let raw: raw::RawConfigs =
            toml::from_str(text).map_err(|source| OzmuxConfigsError::ParseToml {
                path: path.to_path_buf(),
                source,
            })?;
        let merged = raw.apply_to(Self::default());
        merged.validate()?;
        Ok(merged)
    }
```

- [ ] **Step 4: Remove `validate` (and any validate-only tests) from `raw.rs`**

Delete the `pub(crate) fn validate(...)` function from `crates/ozmux_configs/src/raw.rs`. Keep `RawConfigs` and `apply_to`. If `raw.rs`'s `#[cfg(test)] mod tests` has a test that calls `validate` directly, move it to the `lib.rs` test module (rewritten to call `OzmuxConfigs::validate`); otherwise leave raw tests as-is.

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test -p ozmux_configs 2>&1 | grep "test result:" && cargo clippy -p ozmux_configs 2>&1 | tail -3`
Expected: all tests pass; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/ozmux_configs/src/lib.rs crates/ozmux_configs/src/raw.rs
git commit -m "refactor(configs): move validate() onto OzmuxConfigs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Migrate `theme` to direct deserialize

**Files:**
- Modify: `crates/ozmux_configs/src/theme.rs` (add `#[serde(default)]` to `Theme`, delete `ThemePatch`, rewrite tests)
- Modify: `crates/ozmux_configs/src/raw.rs` (field type + `apply_to` line)

**Interfaces:**
- Consumes: `Theme` (existing resolved type with `impl Default`).
- Produces: `Theme` deserializes directly; `RawConfigs.theme: Option<Theme>`.

- [ ] **Step 1: Write the failing test (theme.rs)**

Replace the `#[cfg(test)] mod tests` body in `crates/ozmux_configs/src/theme.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_theme_fills_missing_from_default() {
        let t: Theme = toml::from_str(r##"accent = "#abcdef""##).unwrap();
        assert_eq!(t.accent, "#abcdef");
        assert_eq!(t.background, Theme::default().background);
        assert_eq!(t.destructive, Theme::default().destructive);
    }

    #[test]
    fn empty_theme_is_default() {
        let t: Theme = toml::from_str("").unwrap();
        assert_eq!(t, Theme::default());
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_configs partial_theme_fills_missing_from_default 2>&1 | tail -8`
Expected: FAIL — `Theme` lacks `#[serde(default)]`, so `toml::from_str` errors on the missing `background`/`foreground`/`border`/`destructive` keys (`missing field`).

- [ ] **Step 3: Add `#[serde(default)]` to `Theme` and delete `ThemePatch`**

In `crates/ozmux_configs/src/theme.rs`, add the attribute to the resolved struct:

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct Theme {
```

Delete the entire `ThemePatch` struct and its `impl ThemePatch` block.

- [ ] **Step 4: Update `raw.rs`**

In `crates/ozmux_configs/src/raw.rs`: change the import `use crate::theme::ThemePatch;` to `use crate::theme::Theme;`. Change the field:

```rust
    pub(crate) theme: Option<Theme>,
```

Change the `apply_to` branch:

```rust
        if let Some(theme) = self.theme {
            base.theme = theme;
        }
```

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test -p ozmux_configs 2>&1 | grep "test result:" && cargo clippy -p ozmux_configs 2>&1 | tail -3`
Expected: all pass; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/ozmux_configs/src/theme.rs crates/ozmux_configs/src/raw.rs
git commit -m "refactor(configs): theme direct deserialize, drop ThemePatch

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Migrate `mouse` to direct deserialize

**Files:**
- Modify: `crates/ozmux_configs/src/mouse.rs` (add `#[serde(default)]` to `MouseConfig`, delete `MousePatch`, rewrite tests)
- Modify: `crates/ozmux_configs/src/raw.rs`
- Modify: `crates/ozmux_configs/src/lib.rs` (`mouse_integration_tests` still uses `RawConfigs`; leave until collapse — but verify it still compiles)

**Interfaces:**
- Consumes: `MouseConfig`, `FineModifier` (existing).
- Produces: `MouseConfig` deserializes directly; `RawConfigs.mouse: Option<MouseConfig>`.

- [ ] **Step 1: Write the failing test (mouse.rs)**

Replace the `#[cfg(test)] mod tests` body in `crates/ozmux_configs/src/mouse.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_expected_values() {
        let cfg = MouseConfig::default();
        assert_eq!(cfg.lines_per_notch, 3);
        assert_eq!(cfg.fine_modifier, FineModifier::Alt);
        assert_eq!(cfg.cells_per_notch, 0.5);
        assert_eq!(cfg.drag_threshold_px, 4.0);
    }

    #[test]
    fn partial_mouse_fills_missing_from_default() {
        let cfg: MouseConfig =
            toml::from_str("lines_per_notch = 5\nclick_drift_px = 12.0").unwrap();
        assert_eq!(cfg.lines_per_notch, 5);
        assert_eq!(cfg.click_drift_px, 12.0);
        assert_eq!(cfg.fine_modifier, FineModifier::Alt);
        assert_eq!(cfg.fine_lines, 1);
    }

    #[test]
    fn fine_modifier_parses_lowercase() {
        let cfg: MouseConfig = toml::from_str(r#"fine_modifier = "ctrl""#).unwrap();
        assert_eq!(cfg.fine_modifier, FineModifier::Ctrl);
    }

    #[test]
    fn unknown_key_is_ignored() {
        let cfg: MouseConfig = toml::from_str("lines_per_notch = 5\nbogus = 1").unwrap();
        assert_eq!(cfg.lines_per_notch, 5);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_configs partial_mouse_fills_missing_from_default 2>&1 | tail -8`
Expected: FAIL — missing fields error without `#[serde(default)]`.

- [ ] **Step 3: Add `#[serde(default)]` to `MouseConfig` and delete `MousePatch`**

In `crates/ozmux_configs/src/mouse.rs`:

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct MouseConfig {
```

Delete the entire `MousePatch` struct and its `impl MousePatch`.

- [ ] **Step 4: Update `raw.rs`**

Change `use crate::mouse::MousePatch;` → `use crate::mouse::MouseConfig;`. Field → `pub(crate) mouse: Option<MouseConfig>,`. Branch:

```rust
        if let Some(mouse) = self.mouse {
            base.mouse = mouse;
        }
```

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test -p ozmux_configs 2>&1 | grep "test result:" && cargo clippy -p ozmux_configs 2>&1 | tail -3`
Expected: all pass (the `lib.rs` `mouse_integration_tests` still use `RawConfigs` and remain green); clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/ozmux_configs/src/mouse.rs crates/ozmux_configs/src/raw.rs
git commit -m "refactor(configs): mouse direct deserialize, drop MousePatch

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Migrate `keyboard` to direct deserialize (deny_unknown_fields)

**Files:**
- Modify: `crates/ozmux_configs/src/keyboard.rs`
- Modify: `crates/ozmux_configs/src/raw.rs`

**Interfaces:**
- Consumes: `KeyboardConfig`, `OptionAsAlt` (existing).
- Produces: `KeyboardConfig` deserializes directly with `deny_unknown_fields`; `RawConfigs.keyboard: Option<KeyboardConfig>`.

- [ ] **Step 1: Write the failing test (keyboard.rs)**

Replace the `#[cfg(test)] mod tests` body in `crates/ozmux_configs/src/keyboard.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_none() {
        assert_eq!(KeyboardConfig::default().option_as_alt, OptionAsAlt::None);
    }

    #[test]
    fn parses_value() {
        let cfg: KeyboardConfig = toml::from_str(r#"option_as_alt = "both""#).unwrap();
        assert_eq!(cfg.option_as_alt, OptionAsAlt::Both);
    }

    #[test]
    fn empty_is_default() {
        let cfg: KeyboardConfig = toml::from_str("").unwrap();
        assert_eq!(cfg, KeyboardConfig::default());
    }

    #[test]
    fn rejects_unknown_value() {
        assert!(toml::from_str::<KeyboardConfig>(r#"option_as_alt = "meta""#).is_err());
    }

    #[test]
    fn rejects_unknown_field() {
        assert!(
            toml::from_str::<KeyboardConfig>(r#"option_as_alt2 = "both""#).is_err(),
            "a misspelled key must error under deny_unknown_fields"
        );
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_configs -p ozmux_configs empty_is_default 2>&1 | tail -8`
Expected: FAIL — `KeyboardConfig` lacks `#[serde(default)]` so an empty table errors on the missing `option_as_alt`.

- [ ] **Step 3: Add attributes to `KeyboardConfig` and delete `KeyboardPatch`**

In `crates/ozmux_configs/src/keyboard.rs`:

```rust
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct KeyboardConfig {
    /// Which Option key(s) act as Meta on macOS.
    pub option_as_alt: OptionAsAlt,
}
```

Delete the `KeyboardPatch` struct and its `impl KeyboardPatch`.

- [ ] **Step 4: Update `raw.rs`**

Change `use crate::keyboard::KeyboardPatch;` → `use crate::keyboard::KeyboardConfig;`. Field → `pub(crate) keyboard: Option<KeyboardConfig>,`. Branch:

```rust
        if let Some(keyboard) = self.keyboard {
            base.keyboard = keyboard;
        }
```

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test -p ozmux_configs 2>&1 | grep "test result:" && cargo clippy -p ozmux_configs 2>&1 | tail -3`
Expected: all pass; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/ozmux_configs/src/keyboard.rs crates/ozmux_configs/src/raw.rs
git commit -m "refactor(configs): keyboard direct deserialize, drop KeyboardPatch

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Migrate `osc_webview` to direct deserialize

**Files:**
- Modify: `crates/ozmux_configs/src/osc_webview.rs`
- Modify: `crates/ozmux_configs/src/raw.rs`

**Interfaces:**
- Consumes: `OscWebviewConfig` (existing, `impl Default` with `enabled: true`).
- Produces: `OscWebviewConfig` deserializes directly; `RawConfigs.osc_webview: Option<OscWebviewConfig>`.

- [ ] **Step 1: Write the failing test (osc_webview.rs)**

Replace the `#[cfg(test)] mod tests` body in `crates/ozmux_configs/src/osc_webview.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled() {
        assert!(OscWebviewConfig::default().enabled);
    }

    #[test]
    fn empty_keeps_default_on() {
        let cfg: OscWebviewConfig = toml::from_str("").unwrap();
        assert!(cfg.enabled, "missing enabled defaults to true via impl Default");
    }

    #[test]
    fn explicit_false_overrides() {
        let cfg: OscWebviewConfig = toml::from_str("enabled = false").unwrap();
        assert!(!cfg.enabled);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_configs empty_keeps_default_on 2>&1 | tail -8`
Expected: FAIL — without `#[serde(default)]`, empty table errors on missing `enabled` (and even with field-absent it would not pick up the non-`Default::default()` `true`).

- [ ] **Step 3: Add `#[serde(default)]` to `OscWebviewConfig` and delete `OscWebviewPatch`**

In `crates/ozmux_configs/src/osc_webview.rs`:

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct OscWebviewConfig {
```

Delete `OscWebviewPatch` and its `impl`.

- [ ] **Step 4: Update `raw.rs`**

Change `use crate::osc_webview::OscWebviewPatch;` → `use crate::osc_webview::OscWebviewConfig;`. Field → `pub(crate) osc_webview: Option<OscWebviewConfig>,`. Branch:

```rust
        if let Some(osc_webview) = self.osc_webview {
            base.osc_webview = osc_webview;
        }
```

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test -p ozmux_configs 2>&1 | grep "test result:" && cargo clippy -p ozmux_configs 2>&1 | tail -3`
Expected: all pass; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/ozmux_configs/src/osc_webview.rs crates/ozmux_configs/src/raw.rs
git commit -m "refactor(configs): osc_webview direct deserialize, drop OscWebviewPatch

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Migrate `ozma` to direct deserialize (add Deserialize to OzmaConfig)

**Files:**
- Modify: `crates/ozmux_configs/src/ozma.rs`
- Modify: `crates/ozmux_configs/src/raw.rs`

**Interfaces:**
- Consumes: `OzmaConfig` (existing; currently the ONLY resolved type WITHOUT `Deserialize` — it must gain it).
- Produces: `OzmaConfig` deserializes directly with `deny_unknown_fields`; `RawConfigs.ozma: Option<OzmaConfig>`.

- [ ] **Step 1: Write the failing test (ozma.rs)**

Replace the `#[cfg(test)] mod tests` body in `crates/ozmux_configs/src/ozma.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_is_none() {
        assert!(OzmaConfig::default().shell.is_none());
    }

    #[test]
    fn parses_shell() {
        let cfg: OzmaConfig = toml::from_str(r#"shell = "/bin/fish""#).unwrap();
        assert_eq!(cfg.shell.as_deref(), Some("/bin/fish"));
    }

    #[test]
    fn empty_is_default() {
        let cfg: OzmaConfig = toml::from_str("").unwrap();
        assert_eq!(cfg, OzmaConfig::default());
    }

    #[test]
    fn rejects_unknown_field() {
        assert!(toml::from_str::<OzmaConfig>(r#"shel = "/bin/fish""#).is_err());
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_configs parses_shell 2>&1 | tail -8`
Expected: FAIL — `OzmaConfig` does not implement `Deserialize` (only `OzmaPatch` does), so `toml::from_str::<OzmaConfig>` does not compile.

- [ ] **Step 3: Add `Deserialize` + attributes to `OzmaConfig` and delete `OzmaPatch`**

In `crates/ozmux_configs/src/ozma.rs`, the `use serde::Deserialize;` import already exists. Change the resolved type:

```rust
/// Resolved Ozma mode settings.
#[derive(Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct OzmaConfig {
    /// Shell program to launch. `None` means "resolve at runtime via `$SHELL`".
    pub shell: Option<String>,
}
```

Delete the `OzmaPatch` struct and its `impl OzmaPatch`.

- [ ] **Step 4: Update `raw.rs`**

Change `use crate::ozma::OzmaPatch;` → `use crate::ozma::OzmaConfig;`. Field → `pub(crate) ozma: Option<OzmaConfig>,`. Branch:

```rust
        if let Some(ozma) = self.ozma {
            base.ozma = ozma;
        }
```

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test -p ozmux_configs 2>&1 | grep "test result:" && cargo clippy -p ozmux_configs 2>&1 | tail -3`
Expected: all pass; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/ozmux_configs/src/ozma.rs crates/ozmux_configs/src/raw.rs
git commit -m "refactor(configs): ozma direct deserialize, drop OzmaPatch

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Migrate `tmux` to direct deserialize (remove deprecated auto_connect — BREAKING)

**Files:**
- Modify: `crates/ozmux_configs/src/tmux.rs`
- Modify: `crates/ozmux_configs/src/raw.rs`

**Interfaces:**
- Consumes: `TmuxConfig` (existing).
- Produces: `TmuxConfig` deserializes directly with `deny_unknown_fields`; `RawConfigs.tmux: Option<TmuxConfig>`. `auto_connect` is no longer accepted.

- [ ] **Step 1: Write the failing test (tmux.rs)**

Replace the `#[cfg(test)] mod tests` body in `crates/ozmux_configs/src/tmux.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_targets_path_tmux_default_socket() {
        let c = TmuxConfig::default();
        assert_eq!(c.program, "tmux");
        assert_eq!(c.socket_name, None);
    }

    #[test]
    fn partial_overrides_program_only() {
        let c: TmuxConfig = toml::from_str(r#"program = "/opt/tmux""#).unwrap();
        assert_eq!(c.program, "/opt/tmux");
        assert_eq!(c.socket_name, None);
    }

    #[test]
    fn deprecated_auto_connect_now_errors() {
        assert!(
            toml::from_str::<TmuxConfig>("auto_connect = true").is_err(),
            "auto_connect is removed; deny_unknown_fields must reject it"
        );
    }

    #[test]
    fn empty_is_default() {
        let c: TmuxConfig = toml::from_str("").unwrap();
        assert_eq!(c, TmuxConfig::default());
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_configs deprecated_auto_connect_now_errors 2>&1 | tail -8`
Expected: FAIL — `TmuxConfig` has no `deny_unknown_fields` yet, so `auto_connect = true` parses as an unknown-but-ignored key (test expects an error).

- [ ] **Step 3: Add attributes to `TmuxConfig` and delete `TmuxPatch`**

In `crates/ozmux_configs/src/tmux.rs`:

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct TmuxConfig {
    /// tmux binary to run (looked up on `PATH` unless absolute).
    pub program: String,
    /// Optional named server socket (`tmux -L <name>`); `None` targets the
    /// default server, which is what a normal CLI `tmux` uses.
    pub socket_name: Option<String>,
}
```

Note: `TmuxConfig` currently has a manual `impl Default` (program = "tmux"). Keep that manual impl and do NOT add `Default` to the derive (deriving + manual impl conflict). If the existing derive line lacks `Default`, leave it absent; the `#[serde(default)]` attribute uses the manual `impl Default`. (Concretely: keep `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]` and the existing `impl Default for TmuxConfig`.)

Delete the entire `TmuxPatch` struct (including the `auto_connect` field) and its `impl TmuxPatch`.

- [ ] **Step 4: Update `raw.rs`**

Change `use crate::tmux::TmuxPatch;` → `use crate::tmux::TmuxConfig;`. Field → `pub(crate) tmux: Option<TmuxConfig>,`. Branch:

```rust
        if let Some(tmux) = self.tmux {
            base.tmux = tmux;
        }
```

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test -p ozmux_configs 2>&1 | grep "test result:" && cargo clippy -p ozmux_configs 2>&1 | tail -3`
Expected: all pass; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/ozmux_configs/src/tmux.rs crates/ozmux_configs/src/raw.rs
git commit -m "refactor(configs)!: tmux direct deserialize, remove deprecated auto_connect

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: Migrate `inactive_pane` to direct deserialize (+ normalize())

**Files:**
- Modify: `crates/ozmux_configs/src/inactive_pane.rs` (add `#[serde(default)]`, add `normalize()` + `norm_unit`, delete `InactivePaneConfigPatch` + `apply_unit`, rewrite tests)
- Modify: `crates/ozmux_configs/src/raw.rs` (call `normalize()` before assigning)

**Interfaces:**
- Consumes: `InactivePaneConfig`, `parse_hex_rgb` (existing private fn — keep).
- Produces: `InactivePaneConfig` deserializes directly; `InactivePaneConfig::normalize(&mut self)` (`pub(crate)`); `RawConfigs.inactive_pane: Option<InactivePaneConfig>`.

- [ ] **Step 1: Write the failing tests (inactive_pane.rs)**

Replace the `#[cfg(test)] mod tests` body in `crates/ozmux_configs/src/inactive_pane.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn from_toml_normalized(s: &str) -> InactivePaneConfig {
        let mut c: InactivePaneConfig = toml::from_str(s).unwrap();
        c.normalize();
        c
    }

    #[test]
    fn defaults_match_expected_values() {
        let cfg = InactivePaneConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.dim, 1.0);
        assert_eq!(cfg.tint_color, "#3a3b45");
        assert_eq!(cfg.tint, 0.85);
        assert_eq!(cfg.tint_color_rgb(), (0x3a, 0x3b, 0x45));
    }

    #[test]
    fn partial_fills_from_default_and_normalizes() {
        let cfg = from_toml_normalized("tint = 0.3");
        assert_eq!(cfg.tint, 0.3);
        assert!(cfg.enabled);
        assert_eq!(cfg.dim, 1.0);
        assert_eq!(cfg.tint_color, "#3a3b45");
    }

    #[test]
    fn unit_fields_clamp() {
        let cfg = from_toml_normalized("dim = 4.0\ntint = -1.0");
        assert_eq!(cfg.dim, 1.0);
        assert_eq!(cfg.tint, 0.0);
    }

    #[test]
    fn nan_unit_falls_back_to_default() {
        let cfg = from_toml_normalized("dim = nan\ntint = nan");
        assert_eq!(cfg.dim, 1.0);
        assert_eq!(cfg.tint, 0.85);
        assert!(cfg.dim.is_finite() && cfg.tint.is_finite());
    }

    #[test]
    fn invalid_tint_color_falls_back() {
        let cfg = from_toml_normalized(r#"tint_color = "not-a-color""#);
        assert_eq!(cfg.tint_color, "#3a3b45");
    }

    #[test]
    fn uppercase_tint_color_normalized() {
        let cfg = from_toml_normalized(r#"tint_color = "#FF00AB""#);
        assert_eq!(cfg.tint_color, "#ff00ab");
        assert_eq!(cfg.tint_color_rgb(), (0xff, 0x00, 0xab));
    }

    #[test]
    fn non_ascii_six_byte_tint_color_falls_back_without_panic() {
        let cfg = from_toml_normalized(r#"tint_color = "#中文""#);
        assert_eq!(cfg.tint_color, "#3a3b45");
        let bad = InactivePaneConfig { tint_color: "#中文".to_string(), ..Default::default() };
        assert_eq!(bad.tint_color_rgb(), (0, 0, 0));
    }

    #[test]
    fn stale_keys_ignored_without_error() {
        let cfg = from_toml_normalized("dim = 0.4\ndesaturate = 0.7\nopacity = 0.6");
        assert_eq!(cfg.dim, 0.4);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_configs partial_fills_from_default_and_normalizes 2>&1 | tail -8`
Expected: FAIL — no `#[serde(default)]` (missing fields error) and no `normalize` method.

- [ ] **Step 3: Add `#[serde(default)]`, `normalize()`, `norm_unit`; delete the patch and `apply_unit`**

In `crates/ozmux_configs/src/inactive_pane.rs`:

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct InactivePaneConfig {
```

Delete the `InactivePaneConfigPatch` struct and its `impl InactivePaneConfigPatch`. Delete the `apply_unit` helper.

Add a `normalize` method to the existing `impl InactivePaneConfig` (after `tint_color_rgb`):

```rust
    /// Clamps unit-range fields to `0.0..=1.0` (NaN falls back to the default),
    /// validates `tint_color` as `#RRGGBB` (invalid falls back to the default),
    /// and lowercases a valid `tint_color`.
    pub(crate) fn normalize(&mut self) {
        let d = Self::default();
        self.dim = norm_unit(self.dim, d.dim);
        self.tint = norm_unit(self.tint, d.tint);
        self.webview_dim = norm_unit(self.webview_dim, d.webview_dim);
        self.webview_desaturate = norm_unit(self.webview_desaturate, d.webview_desaturate);
        if parse_hex_rgb(&self.tint_color).is_some() {
            self.tint_color = self.tint_color.to_ascii_lowercase();
        } else {
            self.tint_color = d.tint_color;
        }
    }
```

Add the free helper near `parse_hex_rgb`:

```rust
/// Returns `v` clamped to `0.0..=1.0`, or `default` when `v` is NaN.
fn norm_unit(v: f32, default: f32) -> f32 {
    if v.is_nan() { default } else { v.clamp(0.0, 1.0) }
}
```

Keep `parse_hex_rgb` exactly as-is (the `is_ascii` guard is load-bearing).

- [ ] **Step 4: Update `raw.rs` to normalize on assignment**

Change `use crate::inactive_pane::InactivePaneConfigPatch;` → `use crate::inactive_pane::InactivePaneConfig;`. Field → `pub(crate) inactive_pane: Option<InactivePaneConfig>,`. Branch:

```rust
        if let Some(mut inactive_pane) = self.inactive_pane {
            inactive_pane.normalize();
            base.inactive_pane = inactive_pane;
        }
```

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test -p ozmux_configs 2>&1 | grep "test result:" && cargo clippy -p ozmux_configs 2>&1 | tail -3`
Expected: all pass; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/ozmux_configs/src/inactive_pane.rs crates/ozmux_configs/src/raw.rs
git commit -m "refactor(configs): inactive_pane direct deserialize + normalize()

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: Migrate `font` to flat schema (drop FontPatch/FacePatch — BREAKING)

**Files:**
- Modify: `crates/ozmux_configs/src/font.rs` (flat `FontConfig`, drop `FontPatch`/`FacePatch`, rewrite tests)
- Modify: `crates/ozmux_configs/src/raw.rs`
- Modify: `src/font.rs` (consumer: field renames + test-fixture schema updates)

**Interfaces:**
- Consumes: nothing new.
- Produces: `FontConfig { size: f32, normal: Option<PathBuf>, bold: Option<PathBuf>, italic: Option<PathBuf>, bold_italic: Option<PathBuf> }` deserializes directly; `RawConfigs.font: Option<FontConfig>`. Consumers read `font.normal`/`font.bold`/`font.italic`/`font.bold_italic`.

- [ ] **Step 1: Write the failing tests (font.rs)**

Replace the entire contents of `crates/ozmux_configs/src/font.rs` with:

```rust
//! Font configuration: the `[font]` section.

use serde::Deserialize;

const DEFAULT_SIZE: f32 = 11.25;

/// Fully-resolved font configuration for the terminal grid.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FontConfig {
    /// Terminal font size in points, matching Alacritty.
    pub size: f32,
    /// Absolute or `~`-prefixed path to the regular-face TTF (Bevy GUI only).
    pub normal: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the bold-face TTF (Bevy GUI only).
    pub bold: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the italic-face TTF (Bevy GUI only).
    pub italic: Option<std::path::PathBuf>,
    /// Absolute or `~`-prefixed path to the bold-italic-face TTF (Bevy GUI only).
    pub bold_italic: Option<std::path::PathBuf>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
            normal: None,
            bold: None,
            italic: None,
            bold_italic: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_size_matches_alacritty() {
        assert_eq!(FontConfig::default().size, 11.25);
    }

    #[test]
    fn empty_is_default() {
        let f: FontConfig = toml::from_str("").unwrap();
        assert_eq!(f, FontConfig::default());
    }

    #[test]
    fn parses_flat_paths() {
        let f: FontConfig = toml::from_str(
            "size = 14.0\nnormal = \"/abs/Regular.ttf\"\nbold = \"/abs/Bold.ttf\"",
        )
        .unwrap();
        assert_eq!(f.size, 14.0);
        assert_eq!(f.normal.as_deref(), Some(std::path::Path::new("/abs/Regular.ttf")));
        assert_eq!(f.bold.as_deref(), Some(std::path::Path::new("/abs/Bold.ttf")));
        assert_eq!(f.italic, None);
        assert_eq!(f.bold_italic, None);
    }

    #[test]
    fn size_override_keeps_paths_none() {
        let f: FontConfig = toml::from_str("size = 18.0").unwrap();
        assert_eq!(f.size, 18.0);
        assert_eq!(f.normal, None);
    }

    #[test]
    fn old_nested_table_form_is_rejected() {
        let err = toml::from_str::<FontConfig>("[normal]\npath = \"/x.ttf\"").is_err();
        assert!(err, "old nested [font.normal] path= form must fail to parse");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_configs parses_flat_paths 2>&1 | tail -8`
Expected: FAIL — the pre-edit file still has `FontPatch`/`FacePatch` and a flat `normal_path` field; after the Step 1 replacement the file references the new shape, so compilation fails until `raw.rs` and `src/font.rs` are updated (do Steps 3-4 next). (If you ran Step 1 already, this command shows the workspace not compiling — proceed.)

- [ ] **Step 3: Update `raw.rs`**

Change `use crate::font::FontPatch;` → `use crate::font::FontConfig;`. Field → `pub(crate) font: Option<FontConfig>,`. Branch:

```rust
        if let Some(font) = self.font {
            base.font = font;
        }
```

- [ ] **Step 4: Update the consumer `src/font.rs` field reads**

In `src/font.rs`, rename the four field reads in `bridge_font_config`:

```rust
    let no_override = font.normal.is_none()
        && font.bold.is_none()
        && font.italic.is_none()
        && font.bold_italic.is_none();
```

and:

```rust
    let regular_bytes = load_face_bytes(font.normal.as_deref(), bundled::REGULAR, FontFace::Regular);
    let bold_bytes = load_face_bytes(font.bold.as_deref(), bundled::BOLD, FontFace::Bold);
    let italic_bytes = load_face_bytes(font.italic.as_deref(), bundled::ITALIC, FontFace::Italic);
    let bold_italic_bytes =
        load_face_bytes(font.bold_italic.as_deref(), bundled::BOLD_ITALIC, FontFace::BoldItalic);
```

- [ ] **Step 5: Update the consumer `src/font.rs` test fixtures to the flat schema**

In `src/font.rs` tests, rewrite every embedded `[font.normal]\npath = "..."` (and `[font.bold]`) fixture to the flat form. There are four fixtures to convert (the last one, `corrupt_bold_path_falls_back_per_face_without_dropping_normal_override`, contains TWO nested tables — convert both):

- `configured_normal_path_overrides_regular_face`: `[font.normal]\npath = "{p}"` → `[font]\nnormal = "{p}"`
- `missing_normal_path_falls_back_to_bundled`: same single-table conversion.
- `normal_path_set_does_not_inherit_to_bold`: same single-table conversion.
- `corrupt_bold_path_falls_back_per_face_without_dropping_normal_override`: `[font.normal]\npath = "{n}"\n[font.bold]\npath = "{b}"` → `[font]\nnormal = "{n}"\nbold = "{b}"`

Example (apply the same shape to all four):

```rust
        writeln!(f, "[font]\nnormal = \"{}\"\n", ttf_path.to_string_lossy()).expect("write toml");
```

- [ ] **Step 6: Run crate tests, workspace check, clippy**

Run: `cargo test -p ozmux_configs 2>&1 | grep "test result:" && cargo check --workspace 2>&1 | tail -3 && cargo clippy -p ozmux_configs 2>&1 | tail -3`
Expected: crate tests pass; workspace compiles (consumer updated); clippy clean.

- [ ] **Step 7: Run the consumer's own font tests**

Run: `cargo test -p ozmux-gui --bin ozmux-gui font:: 2>&1 | grep "test result:"`
Expected: the `src/font.rs` font-bridge tests pass against the flat schema. (If the binary target name differs, run `cargo test font::` and confirm the font module tests pass.)

- [ ] **Step 8: Commit**

```bash
git add crates/ozmux_configs/src/font.rs crates/ozmux_configs/src/raw.rs src/font.rs
git commit -m "refactor(configs)!: flat font schema, drop FontPatch/FacePatch

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: Collapse RawConfigs into OzmuxConfigs

**Files:**
- Modify: `crates/ozmux_configs/src/lib.rs` (derive `Deserialize` on `OzmuxConfigs`, rewrite `load_with_env`, add `normalize()`, delete `parse_and_validate`, migrate integration tests, drop `mod raw`)
- Delete: `crates/ozmux_configs/src/raw.rs`

**Interfaces:**
- Consumes: every section resolved type (all now `Deserialize` + `#[serde(default)]`), `Shortcuts`, `StartupMode`.
- Produces: `OzmuxConfigs` derives `Deserialize` with `#[serde(default, deny_unknown_fields)]`; `OzmuxConfigs::normalize(&mut self)` (private); load pipeline `parse → normalize → validate` inside `load_with_env`. `RawConfigs` no longer exists.

- [ ] **Step 1: Write the failing tests (lib.rs)**

In `crates/ozmux_configs/src/lib.rs`, replace `#[cfg(test)] mod mouse_integration_tests` with a `#[cfg(test)] mod integration_tests` that deserializes `OzmuxConfigs` directly and exercises the migrated raw.rs cross-section cases plus the spec-review regression tests:

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;

    fn parse(s: &str) -> OzmuxConfigs {
        let mut c: OzmuxConfigs = toml::from_str(s).unwrap();
        c.normalize();
        c
    }

    #[test]
    fn empty_toml_is_all_defaults() {
        assert_eq!(parse("").font, OzmuxConfigs::default().font);
        assert_eq!(parse("").mouse, OzmuxConfigs::default().mouse);
    }

    #[test]
    fn parses_full_mouse_section() {
        let c = parse(
            "[mouse]\nlines_per_notch = 5\nfine_modifier = \"ctrl\"\nfine_lines = 2\nmax_protocol_events_per_frame = 16\n",
        );
        assert_eq!(c.mouse.lines_per_notch, 5);
        assert_eq!(c.mouse.fine_modifier, mouse::FineModifier::Ctrl);
        assert_eq!(c.mouse.fine_lines, 2);
        assert_eq!(c.mouse.max_protocol_events_per_frame, 16);
    }

    #[test]
    fn missing_section_uses_defaults() {
        assert_eq!(parse("").mouse, mouse::MouseConfig::default());
    }

    #[test]
    fn unknown_top_level_section_is_rejected() {
        assert!(
            toml::from_str::<OzmuxConfigs>("[shortucts]\n").is_err(),
            "a misspelled section name must error under top-level deny_unknown_fields"
        );
    }

    #[test]
    fn old_nested_font_table_fails_at_top_level() {
        assert!(
            toml::from_str::<OzmuxConfigs>("[font.normal]\npath = \"/x.ttf\"").is_err(),
            "old nested font schema must fail to load through OzmuxConfigs, not be shimmed"
        );
    }

    #[test]
    fn inactive_pane_is_normalized_through_pipeline() {
        let c = parse("[inactive_pane]\ndim = 4.0\ntint_color = \"#FF00AB\"");
        assert_eq!(c.inactive_pane.dim, 1.0);
        assert_eq!(c.inactive_pane.tint_color, "#ff00ab");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_configs unknown_top_level_section_is_rejected 2>&1 | tail -8`
Expected: FAIL — `OzmuxConfigs` does not derive `Deserialize` yet, so `toml::from_str::<OzmuxConfigs>` does not compile.

- [ ] **Step 3: Make `OzmuxConfigs` deserializable and add `normalize`**

In `crates/ozmux_configs/src/lib.rs`, add `serde::Deserialize` to the import block (`use serde::Deserialize;`) and change the struct:

```rust
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct OzmuxConfigs {
```

(The field list is unchanged — they are all resolved types now.)

Add the private orchestrator to `impl OzmuxConfigs` (next to `validate`):

```rust
    fn normalize(&mut self) {
        self.inactive_pane.normalize();
    }
```

- [ ] **Step 4: Rewrite `load_with_env` and delete `parse_and_validate`**

Replace `load_with_env` and remove `parse_and_validate`:

```rust
    fn load_with_env(env: &dyn path::Env) -> OzmuxConfigsResult<Self> {
        let configured_path = path::resolve_config_path(env)?;
        tracing::info!(path = %configured_path.display(), "resolving ozmux config path");

        let text = match std::fs::read_to_string(&configured_path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(
                    path = %configured_path.display(),
                    "ozmux config not found; using defaults"
                );
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(OzmuxConfigsError::Io {
                    path: configured_path,
                    source,
                });
            }
        };

        let mut configs: OzmuxConfigs =
            toml::from_str(&text).map_err(|source| OzmuxConfigsError::ParseToml {
                path: configured_path.clone(),
                source,
            })?;
        configs.normalize();
        configs.validate()?;
        Ok(configs)
    }
```

- [ ] **Step 5: Delete `raw.rs` and its module declaration**

Delete the file `crates/ozmux_configs/src/raw.rs`. Remove the `mod raw;` line from `crates/ozmux_configs/src/lib.rs`. Confirm no remaining references:

Run: `grep -rn "raw::\|mod raw\|RawConfigs" crates/ozmux_configs/src/`
Expected: no matches.

- [ ] **Step 6: Run full crate tests, workspace check, clippy, fmt**

Run: `cargo test -p ozmux_configs 2>&1 | grep "test result:" && cargo check --workspace 2>&1 | tail -3 && cargo clippy -p ozmux_configs 2>&1 | tail -3 && cargo fmt -p ozmux_configs`
Expected: all tests pass; workspace compiles; clippy clean; fmt makes no further changes (or only formatting).

- [ ] **Step 7: Verify the full load path via test_support**

Run: `cargo test -p ozmux_configs --features test_support 2>&1 | grep "test result:"`
Expected: pass (exercises `load_with_overrides` → `load_with_env` end-to-end if such tests exist; otherwise this is a no-op confirming the feature still builds).

- [ ] **Step 8: Commit**

```bash
git add crates/ozmux_configs/src/lib.rs
git rm crates/ozmux_configs/src/raw.rs
git commit -m "refactor(configs): collapse RawConfigs into OzmuxConfigs direct deserialize

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**
- Drop `RawConfigs` + all `*Patch` — Tasks 3-11 (each patch deleted in its section task; `RawConfigs` in Task 11). ✓
- `OzmuxConfigs` + sections derive `Deserialize` directly — Tasks 3-11. ✓
- Container `#[serde(default)]` for per-field defaults — every section task Step 3. ✓
- `deny_unknown_fields` preserved per section — keyboard (T5), ozma (T7), tmux (T8), top-level (T11); absent on theme/mouse/osc_webview/inactive_pane/font. ✓
- Font flat schema + consumer + fixtures — Task 10. ✓
- `inactive_pane` normalize() behavior parity — Task 9 (clamp/NaN/hex/lowercase/fallback tests). ✓
- tmux `auto_connect` removal (breaking) — Task 8. ✓
- `validate()` private method on `OzmuxConfigs`, called in `load_with_env` — Task 2 (method) + Task 11 (call site moves into `load_with_env`). ✓
- `OzmaConfig` gains `Deserialize` (spec-review note) — Task 7 Step 3. ✓
- Top-level regression test for old nested font (spec-review note) — Task 11 Step 1 `old_nested_font_table_fails_at_top_level`. ✓
- Private normalize/validate tested in lib.rs test module; full path via test_support; no `parse_and_validate` helper (spec-review note) — Task 11 (tests in `lib.rs`, `parse_and_validate` deleted). ✓
- No external impact beyond `src/font.rs` — only Task 10 touches outside the crate. ✓

**Placeholder scan:** No TBD/TODO/"handle edge cases"/"similar to Task N". Each code step shows full code. ✓

**Type consistency:** Field/method names consistent across tasks — `normalize(&mut self)` (T9 on `InactivePaneConfig`, T11 on `OzmuxConfigs`), `validate(&self)` (T2), `FontConfig` flat fields `normal`/`bold`/`italic`/`bold_italic` (T10) matched by consumer reads (T10 Step 4) and tests. `RawConfigs.<section>: Option<ResolvedType>` with full-replace `apply_to` consistent across T3-T10. ✓
