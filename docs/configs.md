# ozmux Configuration

ozmux reads its configuration from a TOML file at startup. If the file does
not exist, all defaults below are used.

## File location

ozmux resolves the config path in this order:

1. `$OZMUX_CONFIG` — used verbatim if set.
2. `$XDG_CONFIG_HOME/ozmux/config.toml` — if `$XDG_CONFIG_HOME` is set.
3. `~/.config/ozmux/config.toml` — the default.

Unknown sections are rejected at startup, as are unknown keys in `[ozma]`,
`[keyboard]`, and `[shortcuts.bindings]`. Unknown keys in `[font]`, `[mouse]`,
and `[inactive_pane]` are silently ignored. Most invalid values are startup
errors too; the few that are silently clamped or reverted are noted inline
below.

## Example config

Every key below shows its **default** value. Keep only the lines you want to
change — omitted keys fall back to these defaults.

```toml
# ~/.config/ozmux/config.toml

[ozma]
# Shell launched in new terminals. Default: the $SHELL environment variable.
# Absolute path; no ~ expansion.
# shell = "/bin/zsh"

[font]
size = 11.25              # f32, logical px. Must be 0 < size <= 200, else startup error.
# Optional font-file overrides for the GUI (absolute path or ~/...).
# Omit to use the bundled JetBrains Mono.
# normal      = "~/Library/Fonts/MyMono-Regular.ttf"
# bold        = "~/Library/Fonts/MyMono-Bold.ttf"
# italic      = "~/Library/Fonts/MyMono-Italic.ttf"
# bold_italic = "~/Library/Fonts/MyMono-BoldItalic.ttf"

[keyboard]
# macOS only. Which Option key sends Meta instead of composing.
option_as_alt = "none"   # "none" | "left" | "right" | "both"

[mouse]
lines_per_notch = 3              # u32. Lines scrolled per wheel notch.
fine_modifier = "alt"            # "alt" | "ctrl" | "shift" | "none". Modifier for fine (slow) scroll.
fine_lines = 1                   # u32. Lines per notch while fine_modifier is held.
cells_per_notch = 0.5            # f32. Vertical wheel accumulation threshold per notch.
axis_lock_ratio = 0.9            # f32 in 0.0..=1.0. Trackpad dominant-axis lock: horizontal scroll kept only when |x|/hypot(x,y) >= this. 0.0 disables; 1.0 = pure-horizontal only.
double_click_timeout_ms = 400    # u32. Max ms between clicks to count as double/triple.
click_drift_px = 8.0             # f32. Max pointer drift (logical px) between clicks of a multi-click.
drag_threshold_px = 4.0          # f32. Pointer travel (logical px) before a press becomes a drag.
divider_grab_tolerance_px = 4.0  # f32. Half-width (logical px) of the pane-divider grab zone.
# --- advanced drag-autoscroll tuning (rarely changed) ---
max_protocol_events_per_frame = 8  # u32. SGR mouse-protocol event cap per frame.
autoscroll_base_period_ms = 50     # u32. Tick interval when drag-scrolling at the pane edge.
autoscroll_min_period_ms = 16      # u32. Floor on the autoscroll interval.
autoscroll_step_ms = 4             # u32. Interval decrement per cell past the edge.
# Mouse numbers are not range-checked; out-of-range values are used as-is.

[inactive_pane]
# Visual treatment of panes that don't have focus. Float fields are clamped
# to 0.0..=1.0 (out-of-range values are silently clamped, not errors).
enabled = true            # bool. Set false to disable all inactive-pane treatment.
dim = 1.0                 # f32 0..=1. Brightness multiplier (1.0 = no dimming).
tint_color = "#3a3b45"    # "#RRGGBB". Background tint target. Invalid hex silently reverts to this default.
tint = 0.85               # f32 0..=1. Tint strength (0 = off, 1 = full tint).
webview_dim = 0.55        # f32 0..=1. Brightness multiplier for inactive webview overlays.
webview_desaturate = 0.6  # f32 0..=1. Desaturation for inactive webviews (0 = full color, 1 = grey).

[shortcuts.bindings]
# Rebind with a chord string, or unbind with "". Two actions may not share
# the same chord (startup error). See "Chord syntax" below.
paste                 = "Cmd+V"
release-webview-focus = "Ctrl+Shift+Escape"
quit                  = "Cmd+Q"
enter-copy-mode       = "Cmd+S"
detach-session        = "Ctrl+Shift+D"
```

## Chord syntax

A chord is zero or more modifiers followed by exactly one key, joined with `+`.

- **Modifiers** (case-insensitive): `Cmd` (also `Command` / `Meta` / `Super`),
  `Ctrl`, `Shift`, `Alt` (also `Opt` / `Option`).
- **Keys**: any single character (letters are case-insensitive), or a named key:
  `Escape` `Space` `Enter` `Tab` `Backspace` `ArrowUp` `ArrowDown` `ArrowLeft`
  `ArrowRight` `Plus`.
- For the `+` key itself, use the token `Plus` (e.g. `Cmd+Plus`).

Examples: `Cmd+Shift+Q`, `Ctrl+Alt+ArrowLeft`, `Cmd+Plus`.

Invalid chords — an empty token (`Cmd+`), an unknown named key (`Cmd+F12`), a
duplicated modifier (`Cmd+Meta+S`), or more than one key (`Cmd+S+T`) — fail at
startup.

## Shortcut actions

| Action | Default | What it does |
| --- | --- | --- |
| `paste` | `Cmd+V` | Paste from the system clipboard. |
| `release-webview-focus` | `Ctrl+Shift+Escape` | Return keyboard focus from a focused webview to the terminal. |
| `quit` | `Cmd+Q` | Quit ozmux. |
| `enter-copy-mode` | `Cmd+S` | Enter copy mode. |
| `detach-session` | `Ctrl+Shift+D` | Detach the current tmux session. |
