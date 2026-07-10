# orzma Configuration

orzma reads its configuration from a TOML settings file at startup, managed
through Bevy's `bevy::settings` (not orzma's own file-loading code). If the
file does not exist, all defaults below are used. There is no hot-reload:
edit the file and restart orzma to pick up changes.

## File location

| OS | Path |
| --- | --- |
| macOS | `~/Library/Preferences/orzma/settings.toml` |
| Linux | `~/.config/orzma/settings.toml` (i.e. `$XDG_CONFIG_HOME/orzma/settings.toml`, defaulting to `~/.config` when `$XDG_CONFIG_HOME` is unset) |
| Windows | `%LOCALAPPDATA%\orzma\settings.toml` |

This is a per-OS platform "preferences directory" convention, not an
orzma-specific setting — on Linux it still follows the standard
`$XDG_CONFIG_HOME` environment variable the same way it always did. There is
no orzma-specific override anymore, though (see `$ORZMA_CONFIG` below).
Always edit `settings.toml` at the path above directly.

### `$ORZMA_CONFIG` and the one-time legacy migration

Earlier orzma versions read `~/.config/orzma/config.toml` directly (with
`$ORZMA_CONFIG` / `$XDG_CONFIG_HOME` overrides). On first launch after
upgrading, if that legacy file exists and migration hasn't happened yet,
orzma automatically converts it to the new schema (see "Schema changes"
below) and writes it to the `settings.toml` location above. A `.migrated`
marker file next to the new settings file prevents re-migration on later
launches. The old legacy file is left in place, untouched — it's just no
longer read after migration.

**`$ORZMA_CONFIG` is honored only for this one-time migration read.** It is
no longer a runtime config override — this is a deliberate regression from
previous versions, where `$ORZMA_CONFIG` was a live escape hatch you could
set to point orzma at an arbitrary config file on every launch. Once
migration has happened (or if you have no legacy file to migrate),
`$ORZMA_CONFIG` has no effect at all; there is no way to make orzma read
config from anywhere but `settings.toml`.

A legacy file that exists but isn't valid TOML is left alone (not migrated,
not deleted) and migration is retried on the next launch once you fix it.

## Unknown keys and invalid values

Config validation is stricter in some ways and much looser in others than
previous versions:

- **Unknown top-level sections and unknown scalar keys are silently
  ignored.** A typo'd section like `[shortucts]` or a stray key like
  `siz = 12` under `[font]` is a silent no-op — not a warning, not an error.
  This is a consequence of how `bevy::settings` loads config (via Rust
  reflection): unrecognized keys never reach orzma's own code, so orzma has
  no way to detect or report them. Double-check your key spelling against
  the example below if a setting doesn't seem to take effect.
- **Unknown action keys under the two binding maps ARE detected.** A typo'd
  action name inside `[shortcuts.bindings]` or `[vi-mode.bindings]` (e.g.
  `qiut = "Cmd+Q"`) logs a startup warning naming the bad key, and only that
  one entry is skipped — every other binding in your config still loads.
- **Invalid values now warn and fall back to a default, instead of failing
  startup.** A malformed chord, an out-of-range font size, an unparseable
  font style, an invalid `[mouse]`/`[keyboard]` enum value, a duplicate
  chord/key, or a leader that shadows a direct binding — none of these stop
  orzma from starting anymore. The specific fallback for each case is noted
  inline below.
- **One exception stays fatal:** an unresolvable `[font]` face *family* — a
  family name that isn't actually installed on the system — is still a
  startup error, since there is no bundled fallback and no way to render the
  terminal without a usable font.

## Example config

Every key below shows its **default** value. Keep only the lines you want to
change — omitted keys (including entries omitted from `[shortcuts.bindings]`
/ `[vi-mode.bindings]`) fall back to these defaults.

```toml
# ~/Library/Preferences/orzma/settings.toml (macOS)
# ~/.config/orzma/settings.toml             (Linux)
# %LOCALAPPDATA%\orzma\settings.toml         (Windows)

[orzma]
# Shell launched in new terminals. Default: the $SHELL environment variable.
# Absolute path; no ~ expansion.
# shell = "/bin/zsh"

[font]
size = 11.25              # f32, logical px. Out of range (must be 0 < size <= 200): warns and falls back to this default.
# Each face is a table of { family, style }. Omit [font] entirely to use the
# bundled JetBrains Mono Nerd Font. A face's `family`, when omitted, inherits
# `normal.family`; its `style`, when omitted, uses the face's default
# (Regular / Bold / Italic / Bold Italic).
#
# `family` is resolved against installed system fonts; a configured family
# that is not installed is still a STARTUP ERROR (no silent fallback — see
# "Unknown keys and invalid values" above). `style` selects a weight + slant:
# standard names (Regular/Bold/Italic/Bold Italic) plus common weights (Thin,
# Light, Medium, SemiBold, ExtraBold, Black, ...) optionally with
# Italic/Oblique. An unknown style token now warns and that face's style
# falls back to none (its default weight/slant), instead of failing startup.
# (Unlike Alacritty, `style` is matched by weight+slant attributes, not by
# exact subfamily name.)
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

[keyboard]
# macOS only. Which Option key sends Meta instead of composing.
option_as_alt = "none"   # "none" | "left" | "right" | "both". Unrecognized value: warns and falls back to "none".

[mouse]
lines_per_notch = 3              # u32. Lines scrolled per wheel notch.
fine_modifier = "alt"            # "alt" | "ctrl" | "shift" | "none". Modifier for fine (slow) scroll. Unrecognized value: warns and falls back to "alt".
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

[scrollback]
seed_lines = 2000   # usize. Lines of tmux history fetched and seeded into scrollback on attach.

[shortcuts]
# The leader for "<Leader>..." bindings. Either a full chord ("Ctrl+A": press
# the chord, then the next key) OR a bare modifier to TAP ("Cmd"/"Ctrl"/"Alt":
# tap the modifier with no other key, then the next key). Defaults to "Cmd", and
# is active only when at least one action is bound to "<Leader>..." — the stock
# defaults below already bind more than two dozen actions to "<Leader>...", so the Cmd tap
# is armed out of the box. Choose a leader distinct from your tmux prefix.
# "Shift" is not allowed as a tap.
#
# Omit this key entirely to use the built-in default ("Cmd" tap). An empty
# string is NOT a way to disable the leader (see "Note on the leader" below
# for why, and for the actual way to turn individual `<Leader>` bindings off).
leader = "Cmd"
# Modifier-tap window (ms): a press+release within this time, with no intervening
# key or mouse press, counts as a tap. Default 300; 0 reverts to 300.
leader_tap_timeout_ms = 300
# Repeat window (ms) for "<Leader:r>..." bindings: after such a binding fires,
# pressing a repeat-marked key again within this window re-fires the action
# without the leader. Each fire re-arms the window. Default 500; 0 disables
# repeat entirely.
repeat_time_ms = 500

[shortcuts.bindings]
# Each action takes ONE value: a direct chord ("Cmd+V"), a leader-scoped
# chord ("<Leader>s" = leader then s), a repeatable leader-scoped chord
# ("<Leader:r>s" = same, but re-fires within repeat_time_ms), or "" to unbind.
# An unknown action name here warns and is skipped; every other binding still
# loads. Rebinding to a chord already used by another action no longer fails
# startup — it warns, and whichever action is listed FIRST in the "Shortcut
# actions" table below keeps the chord; the other is unbound.

# --- existing actions ---
paste                 = "Cmd+V"        # Standard terminal paste; set paste = "<Leader>p" for the tmux-style leader binding.
copy                  = "Cmd+C"        # Copy the focused terminal's selection to the system clipboard.
release-webview-focus = "<Leader>u"
quit                  = "Cmd+Q"
enter-vi-mode         = "<Leader>s"    # Both modes: Alacritty vi mode in Default, tmux copy-mode under tmux.
detach-session        = "<Leader>x"    # tmux mode only.

# --- pane actions (tmux mode only) ---
select-left-pane      = "<Leader>h"    # select-pane -L
select-down-pane      = "<Leader>j"    # select-pane -D
select-up-pane        = "<Leader>k"    # select-pane -U
select-right-pane     = "<Leader>l"    # select-pane -R
split-vertical-pane   = "<Leader>i"    # split-window -h (side-by-side)
split-horizontal-pane = "<Leader>o"    # split-window -v (stacked)
kill-pane             = "<Leader>p"    # kill-pane, after a confirm prompt
zoom-pane             = "<Leader>z"    # resize-pane -Z
resize-left-pane      = "<Leader:r>Shift+H"  # resize-pane -L 5 (repeatable)
resize-down-pane      = "<Leader:r>Shift+J"  # resize-pane -D 5 (repeatable)
resize-up-pane        = "<Leader:r>Shift+K"  # resize-pane -U 5 (repeatable)
resize-right-pane     = "<Leader:r>Shift+L"  # resize-pane -R 5 (repeatable)

# --- window actions (tmux mode only) ---
new-window            = "<Leader>c"        # new-window
kill-window           = "<Leader>Shift+X"  # kill-window, after a confirm prompt
next-window           = "<Leader>]"        # next-window
previous-window       = "<Leader>["        # previous-window
next-session          = "<Leader>Shift+]"  # switch-client -n (next session)
previous-session      = "<Leader>Shift+["  # switch-client -p (previous session)
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

[vi-mode.bindings]
# Shared vi-mode key bindings, used in BOTH modes: Default mode's Alacritty
# vi mode and tmux mode's copy-mode. Values are ALWAYS an array of key
# strings now, even for a single key (e.g. `line-start = ["0"]`) — see
# "Vi-mode keys" below for the key syntax, duplicate-key rule, and
# mode-coverage caveats. An unknown action name here warns and is skipped.

# --- cursor motion (Default: ViMotion / tmux: send-keys -X) ---
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
toggle-rect-selection = ["Ctrl+V"]          # Block  / rectangle-toggle (works in both modes)

# --- copy / exit ---
yank = ["y", "Enter"]                       # copy the selection, then leave vi mode
exit = ["q", "Escape", "Ctrl+C"]            # leave vi mode

# --- search / jump (tmux mode only — see "Mode coverage" below) ---
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
duplicated modifier (`Cmd+Meta+S`), or more than one key (`Cmd+S+T`) — no
longer fail startup. For a `[shortcuts.bindings]` entry, orzma logs a
warning and skips just that one binding; for `leader`, orzma logs a warning
and falls back to the built-in default leader. Either way, the rest of your
config loads normally.

## Repeatable bindings (`<Leader:r>`)

Binding an action with `<Leader:r>` instead of `<Leader>` makes it repeatable,
like tmux's `bind -r`: after the binding fires, pressing any repeat-marked key
again within `repeat_time_ms` (default 500) re-fires its action without
re-pressing the leader, and each fire re-arms the window. Holding the key down
keeps firing (OS key auto-repeat participates). Any other key — including keys
bound with plain `<Leader>` — closes the window immediately and is handled
normally (it is never swallowed). Pressing the leader inside the window starts
a fresh leader sequence.

Caveat: with a letter key (say `<Leader:r>h`), typing that same letter into the
shell within the window re-fires the action instead of reaching the terminal.
If that bites, set `repeat_time_ms = 0` (disables repeat globally) or drop the
`:r` marker from that binding.

## Shortcut actions

Each row below is one entry you can set under `[shortcuts.bindings]` (see the
example above). The table also fixes the tie-break order used when two
actions end up bound to the same chord (see "Note on the leader" below): the
action listed first wins.

| Action | Default | What it does |
| --- | --- | --- |
| `paste` | `Cmd+V` | Paste from the system clipboard. |
| `copy` | `Cmd+C` | Copy the focused terminal's selection to the system clipboard. |
| `release-webview-focus` | `<Leader>u` | Return keyboard focus from a focused webview to the terminal. |
| `quit` | `Cmd+Q` | Quit orzma. |
| `enter-vi-mode` | `<Leader>s` | Enter vi mode. |
| `detach-session` | `<Leader>x` | Detach the current tmux session (tmux mode only). |
| `select-left-pane` | `<Leader>h` | Focus the pane to the left (tmux mode only). |
| `select-down-pane` | `<Leader>j` | Focus the pane below (tmux mode only). |
| `select-up-pane` | `<Leader>k` | Focus the pane above (tmux mode only). |
| `select-right-pane` | `<Leader>l` | Focus the pane to the right (tmux mode only). |
| `resize-left-pane` | `<Leader:r>Shift+H` | Resize the active pane's border left by 5 cells, repeatable (tmux mode only). |
| `resize-down-pane` | `<Leader:r>Shift+J` | Resize the active pane's border down by 5 cells, repeatable (tmux mode only). |
| `resize-up-pane` | `<Leader:r>Shift+K` | Resize the active pane's border up by 5 cells, repeatable (tmux mode only). |
| `resize-right-pane` | `<Leader:r>Shift+L` | Resize the active pane's border right by 5 cells, repeatable (tmux mode only). |
| `split-vertical-pane` | `<Leader>i` | Split the active pane side-by-side (tmux `split-window -h`) (tmux mode only). |
| `split-horizontal-pane` | `<Leader>o` | Split the active pane stacked (tmux `split-window -v`) (tmux mode only). |
| `kill-pane` | `<Leader>p` | Kill the active pane, after a confirm prompt (tmux mode only). |
| `zoom-pane` | `<Leader>z` | Toggle zoom on the active pane (tmux mode only). |
| `new-window` | `<Leader>c` | Open a new window (tmux mode only). |
| `kill-window` | `<Leader>Shift+X` | Kill the active window, after a confirm prompt (tmux mode only). |
| `next-window` | `<Leader>]` | Switch to the next window (tmux mode only). |
| `previous-window` | `<Leader>[` | Switch to the previous window (tmux mode only). |
| `next-session` | `<Leader>Shift+]` | Switch to the next session (tmux mode only). |
| `previous-session` | `<Leader>Shift+[` | Switch to the previous session (tmux mode only). |
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

Note: some actions only take effect in one mode. `enter-vi-mode` now works in
**both** modes: Alacritty vi mode in Default (single-terminal) mode, tmux
copy-mode under tmux. `detach-session` and all 30 pane/window/session/rename actions
above (`select-*-pane`, `split-*-pane`, `kill-pane`, `zoom-pane`,
`resize-*-pane`, `new-window`, `kill-window`, `next-window`, `previous-window`,
`next-session`, `previous-session`, `select-window-0`…`9`, `rename-window`, `rename-session`) are active under tmux
mode only — they are
inert in Default mode. `paste`, `copy`, `quit`, and `release-webview-focus` work in
both modes. This applies whether an action is bound directly or as a
leader-scoped key (e.g. `<Leader>s`), and regardless of whether the leader is
a chord or a modifier tap.

Note on tmux.conf bindings: with the tmux prefix key no longer intercepted by
orzma, root-table and prefix-table bindings from your `tmux.conf` do not fire
inside orzma — the tmux prefix key passes straight through to the pane like
any other keystroke. Use the `[shortcuts.bindings]` actions above instead. Keys pressed
**inside vi mode** (Default mode's Alacritty vi mode and tmux mode's
copy-mode alike) no longer follow tmux's own copy-mode key tables at all:
orzma resolves every vi-mode key itself from the `[vi-mode.bindings]` table, so
your `tmux.conf` copy-mode / copy-mode-vi customizations and the tmux
`mode-keys` option have no effect inside orzma. See "Vi-mode keys" below.

Note on the leader: because the stock defaults above bind more than two dozen
actions to `<Leader>...`, the `Cmd` tap leader is armed by default — tapping and
releasing `Cmd` (with no other key/mouse press in between) arms the leader,
and the very next keystroke either fires a bound `<Leader>` action or is
swallowed if nothing matches. `LeaderPending` has no expiry: after an
accidental tap, the next keystroke is consumed one way or the other, it does
not time out on its own.

Two consequences of the stock `<Leader>` defaults worth knowing:

- **Rebinding a `<Leader>` chord that a stock default already uses** (e.g.
  `enter-vi-mode = "<Leader>c"`, which collides with the default
  `new-window = "<Leader>c"`) no longer fails startup. It now warns, and
  whichever action is listed FIRST in the table above keeps the chord — here
  `enter-vi-mode` precedes `new-window`, so `enter-vi-mode` wins and
  `new-window` is unbound automatically. If you want both actions reachable,
  unbind the stock default explicitly (`new-window = ""`) or pick a free
  chord instead of relying on the tie-break.
- **An empty `leader` no longer disables `<Leader>`-bound actions.** Unlike
  previous versions, `leader = ""` is now treated as an *invalid* value — it
  warns and falls back to the built-in default leader (`Cmd` tap) instead of
  turning the leader off. There is currently no single setting that disables
  the leader; to get the same effect, unbind every `<Leader>`-bound action
  you don't want individually under `[shortcuts.bindings]` (or rebind the
  ones you need to direct chords, e.g. `next-window = "Cmd+]"`).

## Vi-mode keys

`enter-vi-mode` (see the table above) drops the active pane into vi mode:
Alacritty vi mode in Default (single-terminal) mode, tmux copy-mode under
tmux. What each keystroke does **once inside vi mode** is a separate key
grammar from `[shortcuts.bindings]`, driven entirely by the `[vi-mode.bindings]` table shown
in the example config above — a flat map of 40 vi-mode actions, each bound to
zero or more keys.

### Key grammar

A `[vi-mode.bindings]` entry is an optional `Ctrl+` prefix plus exactly one key.

- **Keys** are either a single character, matched **case-sensitively**
  (`"w"` and `"W"` are different bindings — Shift is expressed through the
  character's case, e.g. `"W"` means Shift+w, not `"Shift+w"`), or one of the
  named keys `Escape` `Enter` `Space` `Tab` `Backspace` `ArrowUp` `ArrowDown`
  `ArrowLeft` `ArrowRight`.
- **`Ctrl+` is the only modifier prefix accepted.** `Cmd+`, `Alt+`, `Shift+`
  (and their aliases) are parse errors inside `[vi-mode.bindings]` — Shift is
  expressed via character case as above, and Cmd/Alt chords are reserved for
  application shortcuts (`[shortcuts.bindings]`); vi-mode gather does not match
  keystrokes with Cmd or Alt held.
- After `Ctrl+`, the key must be an ASCII alphanumeric character or a named
  key — `Ctrl+$` is a parse error. `Ctrl+` entries match on the physical key
  pressed (not the character it produces), so they behave the same regardless
  of layout or case.
- **Values are always an array** of key strings (`yank = ["Y"]`; `exit =
  ["q", "Escape", "Ctrl+C"]`) — even a single key must be written as a
  one-element array. Any action can be bound to zero, one, or several keys.
- **`[]` unbinds** an action (see `search-forward = []` in the fixture-style
  examples below).
- **Duplicate keys now warn instead of failing startup**: if the same key
  string is bound to more than one `[vi-mode.bindings]` action, orzma logs a
  warning naming every colliding action and keeps the key on whichever
  action is listed FIRST in the "Vi-mode actions" table below; every other
  colliding action just loses that one key (its remaining keys, if any, stay
  bound). Unknown action names under `[vi-mode.bindings]` also warn now
  (instead of failing startup) and that one entry is skipped; everything
  else in your config still loads.

Shadowing note: `[shortcuts.bindings]` chords (both leader-scoped and direct) are
matched **before** `[vi-mode.bindings]` keys. If the same keystroke is bound in both
tables, the `[shortcuts.bindings]` action always fires and the `[vi-mode.bindings]` binding
never sees it — e.g. setting `leader = "Ctrl+B"` shadows the default
`page-up = "Ctrl+B"` binding while vi mode is active. orzma does not
validate across the two tables; check your own bindings for overlap.

### Vi-mode actions

| Action | Default | Mode | What it does |
| --- | --- | --- | --- |
| `cursor-left` | `h`, `ArrowLeft` | both | Move the cursor one cell left. |
| `cursor-down` | `j`, `ArrowDown` | both | Move the cursor one cell down. |
| `cursor-up` | `k`, `ArrowUp` | both | Move the cursor one cell up. |
| `cursor-right` | `l`, `ArrowRight` | both | Move the cursor one cell right. |
| `line-start` | `0` | both | Jump to column 0. |
| `line-end` | `$` | both | Jump to the last column. |
| `line-first-char` | `^` | both | Jump to the first non-blank column. |
| `next-word` | `w` | both | Jump to the next (semantic) word start. |
| `previous-word` | `b` | both | Jump to the previous (semantic) word start. |
| `next-word-end` | `e` | both | Jump to the next (semantic) word end. |
| `next-space` | `W` | both | Jump to the next space-delimited word start. |
| `previous-space` | `B` | both | Jump to the previous space-delimited word start. |
| `next-space-end` | `E` | both | Jump to the next space-delimited word end. |
| `screen-top` | `H` | both | Jump to the top visible line. |
| `screen-middle` | `M` | both | Jump to the middle visible line. |
| `screen-bottom` | `L` | both | Jump to the bottom visible line. |
| `previous-paragraph` | `{` | both | Jump to the previous paragraph boundary. |
| `next-paragraph` | `}` | both | Jump to the next paragraph boundary. |
| `matching-bracket` | `%` | both | Jump to the matching bracket. |
| `history-top` | `g` | both | Scroll to the oldest history line. |
| `history-bottom` | `G` | both | Scroll to the live tail. |
| `page-up` | `Ctrl+B` | both | Scroll one page up. |
| `page-down` | `Ctrl+F` | both | Scroll one page down. |
| `half-page-up` | `Ctrl+U` | both | Scroll half a page up. |
| `half-page-down` | `Ctrl+D` | both | Scroll half a page down. |
| `scroll-up` | `Ctrl+Y` | both | Scroll one line up. |
| `scroll-down` | `Ctrl+E` | both | Scroll one line down. |
| `toggle-selection` | `v`, `Space` | both | Toggle a character-wise selection. |
| `toggle-line-selection` | `V` | both | Toggle a line-wise selection. |
| `toggle-rect-selection` | `Ctrl+V` | both | Toggle a rectangular (block) selection. |
| `yank` | `y`, `Enter` | both | Copy the selection to the clipboard and leave vi mode. |
| `exit` | `q`, `Escape`, `Ctrl+C` | both | Leave vi mode. |
| `search-forward` | `/` | tmux only | Open the search-down prompt. |
| `search-backward` | `?` | tmux only | Open the search-up prompt. |
| `search-next` | `n` | tmux only | Repeat the previous search. |
| `search-previous` | `N` | tmux only | Repeat the previous search, reversed. |
| `jump-forward` | `f` | tmux only | Open the jump-to-char-forward prompt. |
| `jump-backward` | `F` | tmux only | Open the jump-to-char-backward prompt. |
| `jump-to-forward` | `t` | tmux only | Open the jump-till-char-forward prompt. |
| `jump-to-backward` | `T` | tmux only | Open the jump-till-char-backward prompt. |

### Mode coverage

The 8 prompt/search actions (`search-forward`, `search-backward`,
`search-next`, `search-previous`, `jump-forward`, `jump-backward`,
`jump-to-forward`, `jump-to-backward` — i.e. the stock `/ ? n N f F t T`
keys) only take effect in **tmux mode**; in Default mode the corresponding
key press is swallowed (no prompt opens, nothing is sent to the pane). Every
other action works in **both** modes, including `toggle-rect-selection`
(`Ctrl+V`), which now toggles a real rectangular selection in both Default
and tmux copy-mode.

### Escape semantics

By default, `Escape` is bound to the `exit` action, which leaves vi mode
entirely. Note that stock tmux binds `Escape` to clear-selection only
(deselecting the current selection without exiting copy mode). To deselect a
selection in orzma without leaving vi mode, press `v` (toggle-selection is a
toggle: with a selection active, it clears it).

Keys not bound to any `[vi-mode.bindings]` action are swallowed while vi mode is
active (they never reach the pane) — this includes stock `copy-mode-vi` keys
that orzma does not carry over by default, such as `:` (goto-line), digit
repeat prefixes, `o` (other-end), `A` (append-and-cancel), `X` / `M-x`
(mark), `;` / `,` (jump repeat), `z` (scroll-middle), and `D`
(copy-end-of-line-and-cancel). Bind them to a `[vi-mode.bindings]` action yourself
if you need them; more built-in actions may be added later.
