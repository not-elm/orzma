# orzma Configuration

orzma reads its configuration from a TOML file at startup. If the file does
not exist, all defaults below are used.

## File location

orzma resolves the config path in this order:

1. `$ORZMA_CONFIG` — used verbatim if set.
2. `$XDG_CONFIG_HOME/orzma/config.toml` — if `$XDG_CONFIG_HOME` is set.
3. `~/.config/orzma/config.toml` — the default.

Unknown sections are rejected at startup, as are unknown keys in `[cursor]`,
`[orzma]`, `[keyboard]`, `[shortcuts]`, `[vi-mode]`, and `[font]`. Unknown keys in
`[mouse]` and `[inactive_pane]` are silently ignored. Most invalid values are
startup errors too; the few that are silently clamped or reverted are noted
inline below.

## Example config

Every key below shows its **default** value. Keep only the lines you want to
change — omitted keys fall back to these defaults.

```toml
# ~/.config/orzma/config.toml

[orzma]
# Shell launched in new terminals. Default: the $SHELL environment variable.
# Absolute path; no ~ expansion.
# shell = "/bin/zsh"
# Whether orzma injects a prompt hook into a recognized shell (pwsh,
# powershell, cmd) so a split pane inherits its working directory. Has
# no effect outside Windows.
shell_integration = true

[cursor]
# Unlike the other enum-valued keys, an unrecognized `style` word silently
# reverts to the default instead of being a startup error.
style = "block"           # block | underline | bar
blink_interval = 750      # milliseconds; 0 keeps the caret steady whatever a program asks for. Any other value below 10 is silently raised to 10.
blink_timeout = 5         # seconds; 0 blinks indefinitely. Silently raised to twice blink_interval (one full on/off cycle) when shorter.
thickness = 0.15          # f32 0..=1, fraction of the cell width. Out-of-range silently clamps; NaN reverts to 0.15; 0 still draws 1 physical px.
unfocused_hollow = true
# The caret starts blinking. DECSCUSR and DECSET 12 / DECRST 12 both
# change it from there, and a DECSCUSR 0 or 7 restores the blink.

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

[keyboard]
# macOS only. Which Option key sends Meta instead of composing.
option_as_alt = "none"   # "none" | "left" | "right" | "both"

[mouse]
lines_per_notch = 3              # u32. Lines scrolled per wheel notch.
fine_modifier = "alt"            # "alt" | "ctrl" | "shift" | "none". Modifier for fine (slow) scroll.
fine_lines = 1                   # u32. Lines per notch while fine_modifier is held.
max_protocol_events_per_frame = 8  # u32. Most wheel notches one routing call turns into mouse reports or alternate-scroll cursor keys, per axis; excess notches are dropped, and cursor keys additionally stop at 240 per call.
cells_per_notch = 0.5            # f32. Wheel accumulation threshold per notch, on both axes.
axis_lock_ratio = 0.9            # f32 in 0.0..=1.0. Trackpad dominant-axis lock: horizontal scroll kept only when |x|/hypot(x,y) >= this. 0.0 disables; 1.0 = pure-horizontal only.
double_click_timeout_ms = 400    # u32. Max ms between clicks to count as double/triple.
click_drift_px = 8.0             # f32. Max pointer drift (logical px) between clicks of a multi-click.
drag_threshold_px = 4.0          # f32. Pointer travel (logical px) before a press becomes a drag.
divider_grab_tolerance_px = 4.0  # f32. Half-width (logical px) of the pane-divider grab zone.
# --- advanced drag-autoscroll tuning (rarely changed) ---
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
# NOTE: the values in this block are the macOS defaults. Seven of them differ on
# Windows and Linux — see "Platform defaults" below for the other table.
# The leader for "<Leader>..." bindings. Either a full chord ("Ctrl+A": press
# the chord, then the next key) OR a bare modifier to TAP ("Cmd"/"Ctrl"/"Alt":
# tap the modifier with no other key, then the next key). Defaults to "Cmd" on
# macOS and "Alt" elsewhere, and is active only when at least one action is
# bound to "<Leader>..." — the stock defaults below already bind more than two
# dozen actions to "<Leader>...", so the tap leader is armed out of the box.
# Set "" to disable it. "Shift" is not allowed as a tap.
leader = "Cmd"
# Modifier-tap window (ms): a press+release within this time, with no intervening
# key or mouse press, counts as a tap. Default 300; 0 reverts to 300.
leader-tap-timeout-ms = 300
# Repeat window (ms) for "<Leader:r>..." bindings: after such a binding fires,
# pressing a repeat-marked key again within this window re-fires the action
# without the leader. Each fire re-arms the window. Default 500; 0 disables
# repeat entirely.
repeat-time-ms = 500

# Each action takes ONE value: a direct chord ("Cmd+V"), a leader-scoped
# chord ("<Leader>s" = leader then s), a repeatable leader-scoped chord
# ("<Leader:r>s" = same, but re-fires within repeat-time-ms), or "" to unbind.
# Rebinding to a chord already used by another action is a startup validation
# error. A direct chord and a "<Leader>"-prefixed chord with the same key
# never collide.

# --- existing actions ---
paste                 = "Cmd+V"        # Standard terminal paste; set paste = "<Leader>p" for a leader binding.
copy                  = "Cmd+C"        # Copy the focused terminal's selection to the system clipboard.
release-webview-focus = "<Leader>u"
quit                  = "Cmd+Q"        # Unbound by default off macOS, where the window manager closes the window.
enter-vi-mode         = "<Leader>s"    # Enters Alacritty vi mode.

# --- pane actions ---
select-left-pane      = "<Leader>h"    # select-pane -L
select-down-pane      = "<Leader>j"    # select-pane -D
select-up-pane        = "<Leader>k"    # select-pane -U
select-right-pane     = "<Leader>l"    # select-pane -R
split-vertical-pane   = "<Leader>i"    # split-window -h (side-by-side)
split-horizontal-pane = "<Leader>o"    # split-window -v (stacked)
kill-pane             = "<Leader>p"    # kill-pane
zoom-pane             = "<Leader>z"    # resize-pane -Z
resize-left-pane      = "<Leader:r>Shift+H"  # resize-pane -L 5 (repeatable)
resize-down-pane      = "<Leader:r>Shift+J"  # resize-pane -D 5 (repeatable)
resize-up-pane        = "<Leader:r>Shift+K"  # resize-pane -U 5 (repeatable)
resize-right-pane     = "<Leader:r>Shift+L"  # resize-pane -R 5 (repeatable)

# --- zoom actions ---
increase-font-size    = "Cmd+Plus"   # Ctrl+Plus off macOS
decrease-font-size    = "Cmd+-"      # Ctrl+- off macOS
reset-font-size       = "Cmd+0"      # Ctrl+0 off macOS

# --- window actions (no effect until the built-in multiplexer lands) ---
new-window            = "<Leader>c"        # new-window
kill-window           = "<Leader>Shift+X"  # kill-window, after a confirm prompt
next-window           = "<Leader>]"        # next-window
previous-window       = "<Leader>["        # previous-window
select-window-0       = "<Leader>0"        # select-window at display index 0
select-window-1       = "<Leader>1"
select-window-2       = "<Leader>2"
select-window-3       = "<Leader>3"
select-window-4       = "<Leader>4"
select-window-5       = "<Leader>5"
select-window-6       = "<Leader>6"
select-window-7       = "<Leader>7"
select-window-8       = "<Leader>8"
select-window-9       = "<Leader>9"

# --- rename action (no effect until the built-in multiplexer lands) ---
rename-window         = "<Leader>r"        # opens the rename prompt for the active window

[vi-mode]
# Vi-mode key bindings for Alacritty vi mode. See "Vi-mode keys" below for
# the key syntax and duplicate-key rule.

# --- cursor motion (trailing comment: ViMotion variant / copy-mode command, for reference) ---
cursor-left        = ["h", "ArrowLeft"]     # Left            / cursor-left
cursor-down        = ["j", "ArrowDown"]     # Down            / cursor-down
cursor-up          = ["k", "ArrowUp"]       # Up              / cursor-up
cursor-right       = ["l", "ArrowRight"]    # Right           / cursor-right
line-start         = ["0"]                  # First           / start-of-line
line-end           = ["$"]                  # Last            / end-of-line
line-first-char    = ["^"]                  # FirstOccupied   / back-to-indentation
next-word          = ["w"]                  # SemanticRight   / next-word
previous-word      = ["b"]                  # SemanticLeft    / previous-word
next-word-end      = ["e"]                  # SemanticRightEnd / next-word-end
next-space         = ["W"]                  # WordRight       / next-space
previous-space     = ["B"]                  # WordLeft        / previous-space
next-space-end     = ["E"]                  # WordRightEnd    / next-space-end
screen-top         = ["H"]                  # High            / top-line
screen-middle      = ["M"]                  # Middle          / middle-line
screen-bottom      = ["L"]                  # Low             / bottom-line
previous-paragraph = ["{"]                  # ParagraphUp     / previous-paragraph
next-paragraph     = ["}"]                  # ParagraphDown   / next-paragraph
matching-bracket   = ["%"]                  # Bracket         / next-matching-bracket

# --- scrolling ---
history-top        = ["g"]                  # Top      / history-top
history-bottom     = ["G"]                  # Bottom   / history-bottom
page-up            = ["Ctrl+B"]             # PageUp   / page-up
page-down          = ["Ctrl+F"]             # PageDown / page-down
half-page-up       = ["Ctrl+U"]             # HalfUp   / halfpage-up
half-page-down     = ["Ctrl+D"]             # HalfDown / halfpage-down
scroll-up          = ["Ctrl+Y"]             # LineUp   / scroll-up
scroll-down        = ["Ctrl+E"]             # LineDown / scroll-down

# --- selection ---
toggle-selection      = ["v", "Space"]      # Simple / begin-selection
toggle-line-selection = ["V"]               # Lines  / select-line
toggle-rect-selection = ["Ctrl+V"]          # Block  / rectangle-toggle

# --- copy / exit ---
yank = ["y", "Enter"]                       # copy the selection, then leave vi mode
exit = ["q", "Escape", "Ctrl+C"]            # leave vi mode

# --- search / jump (currently has no effect — see "Mode coverage" below) ---
search-forward     = ["/"]                  # opens a prompt -> -X search-forward
search-backward    = ["?"]                  # opens a prompt -> -X search-backward
search-next        = ["n"]                  # repeats the last search -> -X search-again
search-previous    = ["N"]                  # repeats it reversed -> -X search-reverse
jump-forward       = ["f"]                  # opens a prompt -> -X jump-forward
jump-backward      = ["F"]                  # opens a prompt -> -X jump-backward
jump-to-forward    = ["t"]                  # opens a prompt -> -X jump-to-forward
jump-to-backward   = ["T"]                  # opens a prompt -> -X jump-to-backward
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

## Repeatable bindings (`<Leader:r>`)

Binding an action with `<Leader:r>` instead of `<Leader>` makes it repeatable,
repeatable: after the binding fires, pressing any repeat-marked key
again within `repeat-time-ms` (default 500) re-fires its action without
re-pressing the leader, and each fire re-arms the window. Holding the key down
keeps firing (OS key auto-repeat participates). Any other key — including keys
bound with plain `<Leader>` — closes the window immediately and is handled
normally (it is never swallowed). Pressing the leader inside the window starts
a fresh leader sequence.

Caveat: with a letter key (say `<Leader:r>h`), typing that same letter into the
shell within the window re-fires the action instead of reaching the terminal.
If that bites, set `repeat-time-ms = 0` (disables repeat globally) or drop the
`:r` marker from that binding.

In vi mode a repeatable binding fires only on the key pressed right after the
leader: the window closes on the next key event, and holding the key does not
keep firing. A second press or an auto-repeat is read as a `[vi-mode]` key
instead — with the stock bindings, `Shift+H` and `Shift+L` jump to the top and
bottom visible line, and `Shift+J` and `Shift+K` do nothing.

## Platform defaults

Seven defaults differ by platform, because macOS has a `Cmd` key and the other
platforms do not. Every other action below is the same everywhere.

| Action | Default (macOS) | Default (Windows / Linux) |
| --- | --- | --- |
| `leader` | `Cmd` (tap) | `Alt` (tap) |
| `paste` | `Cmd+V` | `Ctrl+V` |
| `copy` | `Cmd+C` | `Ctrl+C` |
| `increase-font-size` | `Cmd+Plus` | `Ctrl+Plus` |
| `decrease-font-size` | `Cmd+-` | `Ctrl+-` |
| `reset-font-size` | `Cmd+0` | `Ctrl+0` |
| `quit` | `Cmd+Q` | unbound |

`quit` ships unbound off macOS because the window manager's own close
shortcut (`Alt+F4` on Windows) already exits orzma. Bind it explicitly if you
want a second way out.

`Ctrl+C` copies only while a selection exists. With nothing selected it is sent
to the shell as the interrupt byte (`0x03`) instead, and the `copy` shortcut
dismisses the selection, so pressing `Ctrl+C` twice copies and then interrupts.
The mouse's own copy-on-release keeps the selection highlighted, so a drag
followed by `Ctrl+C` still copies. A copy chord that carries any other
modifier — including the macOS `Cmd+C` default — always copies and never
reaches the shell.

While an application tracks the mouse (for example nvim with `mouse` set),
clicks and drags go to the application instead of orzma's own selection;
holding Shift as the button goes down makes that click or drag select in
orzma instead, and a drag copies on release. Shift only matters at the
press: pressing or letting go of it mid-drag does not reroute the drag. A
pane scrolled back into its history always selects in orzma too, whether
or not Shift is held.

Two stock `[vi-mode]` keys share a chord with these defaults. Inside vi mode
`Ctrl+V` toggles a rectangular selection (the paste action is inert there
anyway), and `Ctrl+C` leaves vi mode whenever there is no selection to copy.

Binding `paste` to `Ctrl+V` does take that key away from the program running in
the terminal, so readline's quoted-insert and vim's visual-block mode no longer
see it. Set `paste = "Ctrl+Shift+V"` to give it back.

## Shortcut actions

The `Default` column lists the macOS value; see "Platform defaults" above for
the seven that differ elsewhere.

| Action | Default | What it does |
| --- | --- | --- |
| `paste` | `Cmd+V` | Paste from the system clipboard. |
| `copy` | `Cmd+C` | Copy the focused terminal's selection to the system clipboard, then dismiss the selection. |
| `increase-font-size` | `Cmd+Plus` / `Ctrl+Plus` | Step the terminal font size up. |
| `decrease-font-size` | `Cmd+-` / `Ctrl+-` | Step the terminal font size down. |
| `reset-font-size` | `Cmd+0` / `Ctrl+0` | Return the terminal font size to `[font] size`. |
| `release-webview-focus` | `<Leader>u` | Return keyboard focus from a focused webview to the terminal. |
| `quit` | `Cmd+Q` | Quit orzma. |
| `enter-vi-mode` | `<Leader>s` | Enter vi mode. |
| `select-left-pane` | `<Leader>h` | Focus the pane to the left. |
| `select-down-pane` | `<Leader>j` | Focus the pane below. |
| `select-up-pane` | `<Leader>k` | Focus the pane above. |
| `select-right-pane` | `<Leader>l` | Focus the pane to the right. |
| `resize-left-pane` | `<Leader:r>Shift+H` | Move a divider of the active pane 5 cells left, repeatable (see "Note on resizing" below). |
| `resize-down-pane` | `<Leader:r>Shift+J` | Move a divider of the active pane 5 cells down, repeatable (see "Note on resizing" below). |
| `resize-up-pane` | `<Leader:r>Shift+K` | Move a divider of the active pane 5 cells up, repeatable (see "Note on resizing" below). |
| `resize-right-pane` | `<Leader:r>Shift+L` | Move a divider of the active pane 5 cells right, repeatable (see "Note on resizing" below). |
| `split-vertical-pane` | `<Leader>i` | Split the active pane side by side (vertical divider); the new pane becomes active. |
| `split-horizontal-pane` | `<Leader>o` | Split the active pane stacked (horizontal divider); the new pane becomes active. |
| `kill-pane` | `<Leader>p` | Kill the active pane; its shell is terminated. |
| `zoom-pane` | `<Leader>z` | Toggle zoom on the active pane (no effect until the built-in multiplexer lands). |
| `new-window` | `<Leader>c` | Open a new window (no effect until the built-in multiplexer lands). |
| `kill-window` | `<Leader>Shift+X` | Kill the active window, after a confirm prompt (no effect until the built-in multiplexer lands). |
| `next-window` | `<Leader>]` | Switch to the next window (no effect until the built-in multiplexer lands). |
| `previous-window` | `<Leader>[` | Switch to the previous window (no effect until the built-in multiplexer lands). |
| `select-window-0` | `<Leader>0` | Switch to the window at index 0 (no effect until the built-in multiplexer lands). |
| `select-window-1` | `<Leader>1` | Switch to the window at index 1 (no effect until the built-in multiplexer lands). |
| `select-window-2` | `<Leader>2` | Switch to the window at index 2 (no effect until the built-in multiplexer lands). |
| `select-window-3` | `<Leader>3` | Switch to the window at index 3 (no effect until the built-in multiplexer lands). |
| `select-window-4` | `<Leader>4` | Switch to the window at index 4 (no effect until the built-in multiplexer lands). |
| `select-window-5` | `<Leader>5` | Switch to the window at index 5 (no effect until the built-in multiplexer lands). |
| `select-window-6` | `<Leader>6` | Switch to the window at index 6 (no effect until the built-in multiplexer lands). |
| `select-window-7` | `<Leader>7` | Switch to the window at index 7 (no effect until the built-in multiplexer lands). |
| `select-window-8` | `<Leader>8` | Switch to the window at index 8 (no effect until the built-in multiplexer lands). |
| `select-window-9` | `<Leader>9` | Switch to the window at index 9 (no effect until the built-in multiplexer lands). |
| `rename-window` | `<Leader>r` | Open the rename prompt for the active window (no effect until the built-in multiplexer lands). |

Note: some actions have no effect yet. `paste`, `copy`, `quit`,
`release-webview-focus`, `enter-vi-mode` (Alacritty vi mode), `select-*-pane`,
`resize-*-pane`, `split-*-pane`, and `kill-pane` all work today through the
built-in multiplexer backend. The remaining 16 window/zoom/rename actions
above (`zoom-pane`, `new-window`, `kill-window`, `next-window`,
`previous-window`, `select-window-0`…`9`, `rename-window`) are no-ops until
the built-in multiplexer grows zoom and window support — the bindings are
accepted and validated at startup, but pressing them does nothing. This
applies whether an action is bound directly or as a leader-scoped key (e.g.
`<Leader>s`), and regardless of whether the leader is a chord or a modifier
tap.

Note on resizing: `resize-*-pane` moves one divider of the active pane 5
cells in the key's direction, picking it the way tmux's `resize-pane` does.
Left and right look at the row of side-by-side panes the active pane belongs
to (up and down at its column of stacked panes): the divider after the pane
moves when there is one, and otherwise the divider before it. So left and
right move the active pane's right border unless the pane is the last one in
its row. Panes nested inside the area that grows or shrinks keep their
proportions; when the active pane is one of them, it changes by only its share
of the 5 cells (possibly none) and its other border can move as well. A
divider stops once the side it shrinks reaches 4 columns or 2 rows per pane
(less when the area the divider splits is too small to give both sides that
much), so a small pane nested on that side can still end up narrower. A
divider never moves against the key, and a key with no divider to move on its
axis does nothing. In vi mode each step needs the leader again (see
"Repeatable bindings" above).

Note on the leader: because the stock defaults above bind more than two dozen
actions to `<Leader>...`, the tap leader is armed by default — tapping and
releasing the leader modifier (`Cmd` on macOS, `Alt` elsewhere, with no other
key/mouse press in between) arms the leader, and the very next keystroke either
fires a bound `<Leader>` action or is swallowed if nothing matches.
`LeaderPending` has no expiry: after an accidental tap, the next keystroke is
consumed one way or the other, it does not time out on its own.

Holding the leader modifier still behaves normally: only a bare press and
release arms the leader, so `Alt+h` continues to reach the shell as a
meta-prefixed key rather than selecting the left pane. Tabbing away from the
window also disarms an in-progress tap.

Two consequences of the stock `<Leader>` defaults worth knowing:

- **Rebinding a `<Leader>` chord that a stock default already uses** (e.g.
  `enter-vi-mode = "<Leader>c"`, which collides with the default
  `new-window = "<Leader>c"`) is a startup validation error naming both
  actions. Unbind the stock default explicitly (`new-window = ""`) or pick a
  free chord.
- **`leader = ""` disables every `<Leader>`-bound action at once** — with the
  stock defaults that includes all 29 leader-bound actions above, silently
  (a warning is logged, but startup succeeds). If you disable the leader,
  rebind the actions you need to direct chords, e.g.
  `next-window = "Ctrl+Shift+]"`.

`Ctrl++` is not a valid value: a chord is split on `+`, so write `Ctrl+Plus`.
A `Plus` binding also fires with Shift held, since `+` is Shift+`=` on a US
layout. Key bindings match physical key positions, so on a non-US layout the
`Plus` and `-` positions may not be where the labels are. The numeric keypad's
`+` and `-` are not bindable. `Plus` resolves to the physical position of the
`=` key on a US layout, and fires whether or not Shift is held — including
when it is the leader. On a layout with a dedicated `+` key, such as German,
that position is a different key, so bind the key you actually want by name
instead.

### Zoom and the window

Zoom never resizes the OS window. The column and row counts change instead,
and the rows are reflowed to the new width, scrollback included: a line that
no longer fits wraps onto the next row, and zooming back out joins it again.
When the window grows taller, or zooming out adds rows, Windows leaves
scrollback in place and adds blank rows at the bottom, because ConPTY keeps no
scrollback; on other platforms rows come back from scrollback.
A full-screen program on the alternate screen is not reflowed; it redraws
itself at the new size.

Zoom scales the terminal grid only. A mounted webview's box grows and shrinks
with the cell pitch, but the page inside keeps its own text size, so zooming
in shows more of the page rather than a larger page. The vi-mode indicator
keeps a fixed size. While a webview owns the keyboard the direct zoom chords
go to the page rather than to orzma: release focus first (`<Leader>u`), or
rebind the zoom actions to `<Leader>`-scoped chords, which fire regardless of
focus.

The zoom factor is not remembered across restarts; set `[font] size` to change
the size permanently.

## Vi-mode keys

`enter-vi-mode` (see the table above) drops the active pane into Alacritty vi
mode on the focused terminal. What each keystroke does **once inside vi
mode** is a separate key grammar from `[shortcuts]`, driven entirely by the
`[vi-mode]` table shown in the example config above — a flat table of 40
vi-mode actions, each bound to zero or more keys.

### Key grammar

A `[vi-mode]` entry is an optional `Ctrl+` prefix plus exactly one key.

- **Keys** are either a single character, matched **case-sensitively**
  (`"w"` and `"W"` are different bindings — Shift is expressed through the
  character's case, e.g. `"W"` means Shift+w, not `"Shift+w"`), or one of the
  named keys `Escape` `Enter` `Space` `Tab` `Backspace` `ArrowUp` `ArrowDown`
  `ArrowLeft` `ArrowRight`.
- **`Ctrl+` is the only modifier prefix accepted.** `Cmd+`, `Alt+`, `Shift+`
  (and their aliases) are parse errors inside `[vi-mode]` — Shift is
  expressed via character case as above, and Cmd/Alt chords are reserved for
  application shortcuts (`[shortcuts]`); vi-mode gather does not match
  keystrokes with Cmd or Alt held.
- After `Ctrl+`, the key must be an ASCII alphanumeric character or a named
  key — `Ctrl+$` is a parse error. `Ctrl+` entries match on the physical key
  pressed (not the character it produces), so they behave the same regardless
  of layout or case.
- **Values** are a single key string (`yank = "Y"`) or an array of key
  strings (`exit = ["q", "Escape", "Ctrl+C"]`); any action can be bound to
  zero, one, or several keys.
- **`""` or `[]` unbinds** an action (see `search-forward = ""` in the
  fixture-style examples below).
- **Duplicate keys are a startup error**: if the same key string is bound to
  more than one `[vi-mode]` action, orzma fails at startup naming every
  colliding action (analogous to `[shortcuts]`'s `DuplicateChords`). Unknown
  action names are rejected the same way unknown `[shortcuts]` actions are.

Shadowing note: `[shortcuts]` chords (both leader-scoped and direct) are
matched **before** `[vi-mode]` keys. If the same keystroke is bound in both
tables, the `[shortcuts]` action normally fires and the `[vi-mode]` binding
never sees it — e.g. setting `leader = "Ctrl+B"` shadows the default
`page-up = "Ctrl+B"` binding while vi mode is active. orzma does not
validate across the two tables; check your own bindings for overlap.

Two actions decline the keystroke instead of shadowing it, so the `[vi-mode]`
binding still runs:

- `paste` does nothing in vi mode, so a direct paste chord passes through —
  the stock `Ctrl+V` reaches `toggle-rect-selection`.
- `copy` passes a Ctrl-only chord through whenever there is no selection to
  copy — the stock `Ctrl+C` reaches `exit`.

### Vi-mode actions

| Action | Default | What it does |
| --- | --- | --- |
| `cursor-left` | `h`, `ArrowLeft` | Move the cursor one cell left. |
| `cursor-down` | `j`, `ArrowDown` | Move the cursor one cell down. |
| `cursor-up` | `k`, `ArrowUp` | Move the cursor one cell up. |
| `cursor-right` | `l`, `ArrowRight` | Move the cursor one cell right. |
| `line-start` | `0` | Jump to column 0. |
| `line-end` | `$` | Jump to the last column. |
| `line-first-char` | `^` | Jump to the first non-blank column. |
| `next-word` | `w` | Jump to the next (semantic) word start. |
| `previous-word` | `b` | Jump to the previous (semantic) word start. |
| `next-word-end` | `e` | Jump to the next (semantic) word end. |
| `next-space` | `W` | Jump to the next space-delimited word start. |
| `previous-space` | `B` | Jump to the previous space-delimited word start. |
| `next-space-end` | `E` | Jump to the next space-delimited word end. |
| `screen-top` | `H` | Jump to the top visible line. |
| `screen-middle` | `M` | Jump to the middle visible line. |
| `screen-bottom` | `L` | Jump to the bottom visible line. |
| `previous-paragraph` | `{` | Jump to the previous paragraph boundary. |
| `next-paragraph` | `}` | Jump to the next paragraph boundary. |
| `matching-bracket` | `%` | Jump to the matching bracket. |
| `history-top` | `g` | Scroll to the oldest history line. |
| `history-bottom` | `G` | Scroll to the live tail. |
| `page-up` | `Ctrl+B` | Scroll one page up. |
| `page-down` | `Ctrl+F` | Scroll one page down. |
| `half-page-up` | `Ctrl+U` | Scroll half a page up. |
| `half-page-down` | `Ctrl+D` | Scroll half a page down. |
| `scroll-up` | `Ctrl+Y` | Scroll one line up. |
| `scroll-down` | `Ctrl+E` | Scroll one line down. |
| `toggle-selection` | `v`, `Space` | Toggle a character-wise selection. |
| `toggle-line-selection` | `V` | Toggle a line-wise selection. |
| `toggle-rect-selection` | `Ctrl+V` | Toggle a rectangular (block) selection. |
| `yank` | `y`, `Enter` | Copy the selection to the clipboard and leave vi mode. |
| `exit` | `q`, `Escape`, `Ctrl+C` | Leave vi mode. |
| `search-forward` | `/` | Open the search-down prompt (currently has no effect). |
| `search-backward` | `?` | Open the search-up prompt (currently has no effect). |
| `search-next` | `n` | Repeat the previous search (currently has no effect). |
| `search-previous` | `N` | Repeat the previous search, reversed (currently has no effect). |
| `jump-forward` | `f` | Open the jump-to-char-forward prompt (currently has no effect). |
| `jump-backward` | `F` | Open the jump-to-char-backward prompt (currently has no effect). |
| `jump-to-forward` | `t` | Open the jump-till-char-forward prompt (currently has no effect). |
| `jump-to-backward` | `T` | Open the jump-till-char-backward prompt (currently has no effect). |

### Mode coverage

The 8 prompt/search actions (`search-forward`, `search-backward`,
`search-next`, `search-previous`, `jump-forward`, `jump-backward`,
`jump-to-forward`, `jump-to-backward` — i.e. the stock `/ ? n N f F t T`
keys) currently have no effect: the key press is swallowed (no prompt opens,
nothing happens) while vi mode is active. Local vi-mode search is a future
feature. Every other action works today, including `toggle-rect-selection`
(`Ctrl+V`), which toggles a real rectangular selection.

### Escape semantics

By default, `Escape` is bound to the `exit` action, which leaves vi mode
entirely. To deselect a selection in orzma without leaving vi mode, press `v`
(toggle-selection is a toggle: with a selection active, it clears it).

Keys not bound to any `[vi-mode]` action are swallowed while vi mode is
active (they never reach the pane) — this includes stock `copy-mode-vi` keys
that orzma does not carry over by default, such as `:` (goto-line), digit
repeat prefixes, `o` (other-end), `A` (append-and-cancel), `X` / `M-x`
(mark), `;` / `,` (jump repeat), `z` (scroll-middle), and `D`
(copy-end-of-line-and-cancel). Bind them to a `[vi-mode]` action yourself
if you need them; more built-in actions may be added later.
