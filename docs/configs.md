# ozmux Configuration

ozmux reads its configuration from a TOML file at startup. If the file does
not exist, all defaults below are used.

## File location

ozmux resolves the config path in this order:

1. `$OZMUX_CONFIG` — used verbatim if set.
2. `$XDG_CONFIG_HOME/ozmux/config.toml` — if `$XDG_CONFIG_HOME` is set.
3. `~/.config/ozmux/config.toml` — the default.

Unknown sections are rejected at startup, as are unknown keys in `[ozma]`,
`[keyboard]`, and `[shortcuts]`. Unknown keys in `[font]`, `[mouse]`,
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

[shortcuts]
# The leader for "<Leader>..." bindings. Either a full chord ("Ctrl+A": press
# the chord, then the next key) OR a bare modifier to TAP ("Cmd"/"Ctrl"/"Alt":
# tap the modifier with no other key, then the next key). Defaults to "Cmd", and
# is active only when at least one action is bound to "<Leader>..." — the stock
# defaults below already bind two dozen actions to "<Leader>...", so the Cmd tap
# is armed out of the box. Set "" to disable it. "Shift" is not allowed as a tap.
# Choose a leader distinct from your tmux prefix.
leader = "Cmd"
# Modifier-tap window (ms): a press+release within this time, with no intervening
# key or mouse press, counts as a tap. Default 300; 0 reverts to 300.
leader-tap-timeout-ms = 300

# Each action takes ONE value: a direct chord ("Cmd+V"), a leader-scoped
# chord ("<Leader>s" = leader then s), or "" to unbind. Rebinding to a chord
# already used by another action is a startup validation error. A direct
# chord and a "<Leader>"-prefixed chord with the same key never collide.

# --- existing actions ---
paste                 = "<Leader>p"    # Was "Cmd+V" pre-tmux-native-shortcuts; set paste = "Cmd+V" to restore.
release-webview-focus = "Ctrl+Shift+Escape"
quit                  = "Cmd+Q"
enter-copy-mode       = "Cmd+S"        # Both modes: Alacritty vi mode in Default, tmux copy-mode under tmux.
detach-session        = "Ctrl+Shift+D" # tmux mode only.

# --- pane actions (tmux mode only) ---
select-left-pane      = "<Leader>h"    # select-pane -L
select-down-pane      = "<Leader>j"    # select-pane -D
select-up-pane        = "<Leader>k"    # select-pane -U
select-right-pane     = "<Leader>l"    # select-pane -R
split-vertical-pane   = "<Leader>i"    # split-window -h (side-by-side)
split-horizontal-pane = "<Leader>o"    # split-window -v (stacked)
kill-pane             = "<Leader>x"    # kill-pane, after a confirm prompt
zoom-pane             = "<Leader>z"    # resize-pane -Z

# --- window actions (tmux mode only) ---
new-window            = "<Leader>c"        # new-window
kill-window           = "<Leader>Shift+X"  # kill-window, after a confirm prompt
next-window           = "<Leader>n"        # next-window
previous-window       = "<Leader>Shift+N"  # previous-window (p is taken by paste)
select-window-0       = "<Leader>0"        # select-window -t @<id at tmux index 0>
select-window-1       = "<Leader>1"
select-window-2       = "<Leader>2"
select-window-3       = "<Leader>3"
select-window-4       = "<Leader>4"
select-window-5       = "<Leader>5"
select-window-6       = "<Leader>6"
select-window-7       = "<Leader>7"
select-window-8       = "<Leader>8"
select-window-9       = "<Leader>9"

# --- rename actions (tmux mode only) ---
rename-window         = "<Leader>r"        # opens the rename prompt for the active window
rename-session        = "<Leader>Shift+R"  # opens the rename prompt for the session
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
| `paste` | `<Leader>p` | Paste from the system clipboard. |
| `release-webview-focus` | `Ctrl+Shift+Escape` | Return keyboard focus from a focused webview to the terminal. |
| `quit` | `Cmd+Q` | Quit ozmux. |
| `enter-copy-mode` | `Cmd+S` | Enter copy mode. |
| `detach-session` | `Ctrl+Shift+D` | Detach the current tmux session (tmux mode only). |
| `select-left-pane` | `<Leader>h` | Focus the pane to the left (tmux mode only). |
| `select-down-pane` | `<Leader>j` | Focus the pane below (tmux mode only). |
| `select-up-pane` | `<Leader>k` | Focus the pane above (tmux mode only). |
| `select-right-pane` | `<Leader>l` | Focus the pane to the right (tmux mode only). |
| `split-vertical-pane` | `<Leader>i` | Split the active pane side-by-side (tmux `split-window -h`) (tmux mode only). |
| `split-horizontal-pane` | `<Leader>o` | Split the active pane stacked (tmux `split-window -v`) (tmux mode only). |
| `kill-pane` | `<Leader>x` | Kill the active pane, after a confirm prompt (tmux mode only). |
| `zoom-pane` | `<Leader>z` | Toggle zoom on the active pane (tmux mode only). |
| `new-window` | `<Leader>c` | Open a new window (tmux mode only). |
| `kill-window` | `<Leader>Shift+X` | Kill the active window, after a confirm prompt (tmux mode only). |
| `next-window` | `<Leader>n` | Switch to the next window (tmux mode only). |
| `previous-window` | `<Leader>Shift+N` | Switch to the previous window (tmux mode only). |
| `select-window-0` | `<Leader>0` | Switch to the window at tmux index 0 (tmux mode only). |
| `select-window-1` | `<Leader>1` | Switch to the window at tmux index 1 (tmux mode only). |
| `select-window-2` | `<Leader>2` | Switch to the window at tmux index 2 (tmux mode only). |
| `select-window-3` | `<Leader>3` | Switch to the window at tmux index 3 (tmux mode only). |
| `select-window-4` | `<Leader>4` | Switch to the window at tmux index 4 (tmux mode only). |
| `select-window-5` | `<Leader>5` | Switch to the window at tmux index 5 (tmux mode only). |
| `select-window-6` | `<Leader>6` | Switch to the window at tmux index 6 (tmux mode only). |
| `select-window-7` | `<Leader>7` | Switch to the window at tmux index 7 (tmux mode only). |
| `select-window-8` | `<Leader>8` | Switch to the window at tmux index 8 (tmux mode only). |
| `select-window-9` | `<Leader>9` | Switch to the window at tmux index 9 (tmux mode only). |
| `rename-window` | `<Leader>r` | Open the rename prompt for the active window (tmux mode only). |
| `rename-session` | `<Leader>Shift+R` | Open the rename prompt for the session (tmux mode only). |

Note: some actions only take effect in one mode. `enter-copy-mode` now works in
**both** modes: Alacritty vi mode in Default (single-terminal) mode, tmux
copy-mode under tmux. `detach-session` and all 24 pane/window/rename actions
above (`select-*-pane`, `split-*-pane`, `kill-pane`, `zoom-pane`, `new-window`,
`kill-window`, `next-window`, `previous-window`, `select-window-0`…`9`,
`rename-window`, `rename-session`) are active under tmux mode only — they are
inert in Default mode. `paste`, `quit`, and `release-webview-focus` work in
both modes. This applies whether an action is bound directly or as a
leader-scoped key (e.g. `<Leader>s`), and regardless of whether the leader is
a chord or a modifier tap.

Note on tmux.conf bindings: with the tmux prefix key no longer intercepted by
ozmux, root-table and prefix-table bindings from your `tmux.conf` do not fire
inside ozmux — the tmux prefix key passes straight through to the pane like
any other keystroke. Use the `[shortcuts]` actions above instead. Keys pressed
**inside tmux copy-mode** are unaffected by this and still follow tmux's own
copy-mode key tables (`copy-mode` / `copy-mode-vi`, selected by the tmux
`mode-keys` option), since ozmux forwards them to tmux once copy-mode is
active.

Note on the leader: because the stock defaults above bind two dozen actions to
`<Leader>...`, the `Cmd` tap leader is armed by default — tapping and
releasing `Cmd` (with no other key/mouse press in between) arms the leader,
and the very next keystroke either fires a bound `<Leader>` action or is
swallowed if nothing matches. `LeaderPending` has no expiry: after an
accidental tap, the next keystroke is consumed one way or the other, it does
not time out on its own.

Two consequences of the stock `<Leader>` defaults worth knowing:

- **Rebinding a `<Leader>` chord that a stock default already uses** (e.g.
  `enter-copy-mode = "<Leader>c"`, which collides with the default
  `new-window = "<Leader>c"`) is a startup validation error naming both
  actions. Unbind the stock default explicitly (`new-window = ""`) or pick a
  free chord.
- **`leader = ""` disables every `<Leader>`-bound action at once** — with the
  stock defaults that includes `paste` and all 24 tmux actions, silently
  (a warning is logged, but startup succeeds). If you disable the leader,
  rebind the actions you need to direct chords, e.g. `paste = "Cmd+V"`.
