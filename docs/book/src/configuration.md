# Configuration

orzma reads its configuration from a TOML file at startup. If the file does
not exist, every setting keeps its default.

## File location

orzma resolves the config path in this order:

1. `$ORZMA_CONFIG` — used verbatim if set.
2. `$XDG_CONFIG_HOME/orzma/config.toml` — if `$XDG_CONFIG_HOME` is set.
3. `~/.config/orzma/config.toml` — the default.

`~` is your home directory on every platform, so on Windows the default is
`%USERPROFILE%\.config\orzma\config.toml`.

## Validation

orzma checks the file when it starts. A problem has one of two effects:

- **orzma ignores the whole file** and starts with every setting at its
  default when the file cannot be read as a configuration: a TOML syntax
  error, an unknown section, an unknown key in `[cursor]`, `[orzma]`,
  `[keyboard]`, `[shortcuts]`, `[vi-mode]`, `[selection]`, or `[font]`, a
  value of the wrong type or an unknown word, or a malformed key binding.
- **orzma does not start** when the settings conflict or cannot be applied: a
  key bound to more than one action, a leader that shadows another binding or
  cannot be used, a font size or style outside the allowed values, or a font
  family that is not installed.

In both cases orzma writes the reason to standard error. To read it, start
orzma from a terminal: on macOS, run `/Applications/orzma.app/Contents/MacOS/orzma`;
on Windows, run `orzma 2> orzma-error.txt` and open the file.

Unknown keys in `[mouse]` and `[inactive_pane]` are silently ignored, and the
few values that are silently clamped or reverted are noted in the comments
below.

## Settings

Every key below shows its default value. Keep only the lines you want to
change; omitted keys fall back to these defaults. Key bindings have their own
pages: the `[shortcuts]` table is described in [Key Bindings](key-bindings.md),
and the `[vi-mode]` table in [Vi Mode](vi-mode.md).

### `[orzma]`

```toml
[orzma]
# Shell launched in new panes. Default: $SHELL when it is set (on Windows,
# only when that program exists); otherwise /bin/sh on macOS, and on Windows
# pwsh or powershell if installed, then %COMSPEC% when it is set, then
# cmd.exe. No ~ expansion.
# shell = "/bin/zsh"
# Whether orzma injects a prompt hook into a recognized shell (pwsh,
# powershell, cmd) so a split pane inherits its working directory. Has
# no effect outside Windows.
shell_integration = true
```

### `[cursor]`

```toml
[cursor]
# Unlike the other enum-valued keys, an unrecognized `style` word silently
# reverts to the default instead of making orzma ignore the whole file.
style = "block"           # block | underline | bar
blink_interval = 750      # milliseconds; 0 keeps the caret steady whatever a program asks for. Any other value below 10 is silently raised to 10.
blink_timeout = 5         # seconds; 0 blinks indefinitely. Silently raised to twice blink_interval (one full on/off cycle) when shorter.
thickness = 0.15          # f32 0..=1, fraction of the cell width. Out-of-range silently clamps; NaN reverts to 0.15; 0 still draws 1 physical px.
unfocused_hollow = true
# The caret starts blinking. DECSCUSR and DECSET 12 / DECRST 12 both
# change it from there, and a DECSCUSR 0 or 7 restores the blink.
```

### `[selection]`

```toml
[selection]
# Characters that end a word for the vi-mode `w` / `b` / `e` motions,
# besides whitespace. Read at startup. Omit the key to keep this default.
semantic_escape_chars = ",│`|:\"' ()[]{}<>\t"
```

### `[font]`

```toml
[font]
size = 11.25              # f32, logical px. Must be 0 < size <= 200, else startup error.
# Each face is a table of { family, style }. Omit [font] entirely to use the
# bundled JetBrains Mono Nerd Font. A face's `family`, when omitted, inherits
# `normal.family`; its `style`, when omitted, uses the face's default
# (Regular / Bold / Italic / Bold Italic).
#
# `family` is resolved against installed system fonts; a configured family that
# is not installed is a STARTUP ERROR (no silent fallback). `style` selects a
# weight + slant: standard names (Regular/Bold/Italic/Bold Italic) plus common
# weights (Thin, Light, Medium, SemiBold, ExtraBold, Black, ...) optionally with
# Italic/Oblique. An unknown style token is a config error. (Unlike Alacritty,
# `style` is matched by weight+slant attributes, not by exact subfamily name.)
#
# [font.normal]
# family = "JetBrains Mono"
# style  = "Regular"
# [font.bold]
# style  = "Bold"                 # inherits family = "JetBrains Mono"
# [font.italic]
# family = "Cascadia Code"
# style  = "Italic"
# [font.bold_italic]
# style  = "Bold Italic"
#
# ui = { family = "Inter", style = "Medium" }
# The UI-chrome face (window bar, prompts, indicators). `family` and `style`
# each inherit from `normal` when omitted (ui.family -> normal.family,
# ui.style -> normal.style). A configured `ui.family` that is not installed is
# a startup error, same as the terminal faces. `style` uses the same weight +
# slant syntax and is applied to UI text. When no family resolves anywhere
# (bundled fallback), style rounds to the nearest of the four bundled faces.
```

### `[keyboard]`

```toml
[keyboard]
# macOS only. Which Option key sends Meta instead of composing.
option_as_alt = "none"   # "none" | "left" | "right" | "both"
```

### `[mouse]`

```toml
[mouse]
lines_per_notch = 3              # u32. Lines scrolled per wheel notch.
fine_modifier = "alt"            # "alt" | "ctrl" | "shift" | "none". Modifier for fine (slow) scroll; "shift" has no effect on macOS.
fine_lines = 1                   # u32. Lines per notch while fine_modifier is held.
max_protocol_events_per_frame = 8  # u32. Most wheel notches one routing call turns into mouse reports or alternate-scroll cursor keys, per axis; excess notches are dropped, and cursor keys additionally stop at 240 per call.
cells_per_notch = 0.5            # f32. Wheel accumulation threshold per notch, on both axes.
axis_lock_ratio = 0.9            # f32, clamped to 0.0..=1.0; a non-finite value reverts to 0.9. Trackpad dominant-axis lock: horizontal scroll kept only when |x|/hypot(x,y) >= this. 0.0 disables; 1.0 = pure-horizontal only.
double_click_timeout_ms = 400    # u32. Max ms between clicks to count as double/triple.
click_drift_px = 8.0             # f32. Max pointer drift (logical px) between clicks of a multi-click.
# The five keys below are accepted but not used yet: changing them has no effect.
drag_threshold_px = 4.0          # f32. Pointer travel (logical px) before a press becomes a drag.
divider_grab_tolerance_px = 4.0  # f32. Half-width (logical px) of the pane-divider grab zone.
# --- advanced drag-autoscroll tuning (rarely changed) ---
autoscroll_base_period_ms = 50     # u32. Tick interval when drag-scrolling at the pane edge.
autoscroll_min_period_ms = 16      # u32. Floor on the autoscroll interval.
autoscroll_step_ms = 4             # u32. Interval decrement per cell past the edge.
# Other mouse numbers are not range-checked; out-of-range values are used as-is.
```

### `[inactive_pane]`

```toml
[inactive_pane]
# Visual treatment of panes that don't have focus. Float fields are clamped
# to 0.0..=1.0 (out-of-range values are silently clamped, not errors).
enabled = true            # bool. Set false to disable all inactive-pane treatment.
dim = 1.0                 # f32 0..=1. Brightness multiplier (1.0 = no dimming).
tint_color = "#3a3b45"    # "#RRGGBB". Background tint target. Invalid hex silently reverts to this default.
tint = 0.85               # f32 0..=1. Tint strength (0 = off, 1 = full tint).
webview_dim = 0.55        # f32 0..=1. Brightness multiplier for inactive webview overlays.
webview_desaturate = 0.6  # f32 0..=1. Desaturation for inactive webviews (0 = full color, 1 = grey).
```
