# Configuration

orzma reads its configuration from a TOML file when it starts. If the file
does not exist, every setting keeps its default. Restart orzma after you
change the file.

## File location

orzma resolves the config path in this order:

1. `$ORZMA_CONFIG` — used verbatim if set and not empty.
2. `$XDG_CONFIG_HOME/orzma/config.toml` — if `$XDG_CONFIG_HOME` is set and
   not empty.
3. `~/.config/orzma/config.toml` — the default.

`~` is your home directory on every platform, so on Windows the default is
`%USERPROFILE%\.config\orzma\config.toml`.

## Validation

orzma checks the file when it starts. A problem has one of two effects:

- **orzma ignores the whole file** and starts with every setting at its
  default when the file cannot be read as a configuration: a TOML syntax
  error, an unknown table, an unknown key in any table except `[mouse]` and
  `[inactive_pane]`, a value of the wrong type or an unknown word, a malformed
  key binding, a file that cannot be read, such as one that is not UTF-8, or
  a home directory that orzma cannot find.
- **orzma does not start** when the settings conflict or cannot be applied: a
  key bound to more than one action, a leader that shadows another binding or
  cannot be used, a font size or style outside the allowed values, or a font
  family that is not installed.

In both cases orzma writes the reason to standard error. To read it, start
orzma from a terminal: on macOS, run `/Applications/orzma.app/Contents/MacOS/orzma`;
on Linux, run `orzma`; on Windows, run `orzma 2> orzma-error.txt` and open the
file.

Unknown keys in `[mouse]` and `[inactive_pane]` are ignored. A few values are
clamped or replaced with their default instead of being rejected; the keys
below say which.

## Settings

Every key is optional: write only the ones you want to change. For example:

```toml
[cursor]
style = "bar"

[font]
size = 13
```

The two key-binding tables, `[shortcuts]` and `[vi-mode]`, are described in
[Key Bindings](key-bindings.md).

### `[orzma]`

#### `shell` {#orzma-shell}

Default: none · A program path

The shell each new pane runs. Without `shell`, or with an empty one, orzma
uses `$SHELL` when it is set (on Windows, only when that program exists);
otherwise `/bin/sh` on macOS and Linux, and on Windows `pwsh` or `powershell`
if installed, then `%COMSPEC%` when it is set, then `cmd.exe`. On macOS,
orzma starts the shell through `zsh`, so `~` and environment variables in the
value are expanded; on Windows and Linux the value is used as written.

```toml
[orzma]
shell = "/opt/homebrew/bin/fish"
```

#### `shell_integration` {#orzma-shell_integration}

Default: `true` · `true` or `false` · Windows only

Whether orzma makes PowerShell (`pwsh`, `powershell`) and `cmd` report their
working directory, so that a split pane starts in the same directory (see
[Working directory of a new pane](panes-and-workspaces.md#working-directory-of-a-new-pane)).
It has no effect on macOS and Linux.

### `[cursor]`

#### `style` {#cursor-style}

Default: `"block"` · `"block"`, `"underline"`, or `"bar"`

The caret shape. Case does not matter. An unknown word keeps the default
instead of making orzma ignore the whole file. Programs can change the shape
while they run (see [Cursor and text attributes](terminal-compatibility.md#cursor-and-text-attributes)).

#### `blink_interval` {#cursor-blink_interval}

Default: `750` · Milliseconds

How long the caret stays lit, and then dark, while it blinks. The caret blinks
from the start, and programs can turn blinking off and on (see [Modes](terminal-compatibility.md#modes)). `0` keeps the caret
steady whatever a program asks for. A value from 1 to 9 is raised to 10.

#### `blink_timeout` {#cursor-blink_timeout}

Default: `5` · Seconds

How long the caret keeps blinking after your last keystroke; then it stays lit
until you type again. `0` keeps it blinking. A value shorter than one full
blink, twice `blink_interval`, is raised to that.

#### `thickness` {#cursor-thickness}

Default: `0.15` · A number from 0 to 1

The thickness of the underline and bar carets, and of the hollow caret's
outline, as a fraction of the cell width. A value outside 0 to 1 is clamped.
The caret is always at least one physical pixel thick.

#### `unfocused_hollow` {#cursor-unfocused_hollow}

Default: `true` · `true` or `false`

Whether the caret of an inactive pane, and of every pane while the orzma
window is not focused, is drawn as a hollow block.

### `[selection]`

#### `semantic_escape_chars` {#selection-semantic_escape_chars}

Default: the characters below · A string

The characters that end a word for the vi mode `w`, `b`, and `e` motions.
Spaces and tabs always end a word as well.

```toml
[selection]
semantic_escape_chars = ",│`|:\"' ()[]{}<>\t"
```

### `[font]`

#### `size` {#font-size}

Default: `11.25` · Logical pixels

The terminal font size. It must be greater than 0 and at most 200; otherwise
orzma does not start. The font size shortcuts change the size while orzma
runs, and `reset-font-size` returns to this value.

#### `normal`, `bold`, `italic`, `bold_italic` {#font-faces}

Default: the bundled JetBrains Mono Nerd Font · Tables of `family` and `style`

The four faces the terminal draws text with. Each is a table with two
optional keys:

- `family` is the name of an installed font family. A face without `family`
  uses `normal.family`. When no face sets a family, orzma draws the terminal
  with its bundled JetBrains Mono Nerd Font and ignores these four faces'
  `style`. A family that is not installed stops orzma from starting.
- `style` is a weight, `Italic` or `Oblique`, or both, as in `"Bold Italic"`,
  `"Medium"`, or `"Italic"`. The weights are `Thin`, `ExtraLight`, `Light`,
  `Regular`, `Medium`, `SemiBold`, `Bold`, `ExtraBold`, and `Black`, and the
  common aliases such as `Normal`, `Book`, `DemiBold`, and `Heavy` work too.
  Case, spaces, and hyphens do not matter, and numeric weights such as `700`
  are not accepted. A face without `style` uses its own: Regular, Bold,
  Italic, or Bold Italic. orzma picks the installed face closest to the weight
  and slant, rather than matching the face's name. An unknown style stops
  orzma from starting.

```toml
[font.normal]
family = "JetBrains Mono"
style = "Regular"

[font.bold]
style = "Bold"            # uses normal's family

[font.italic]
family = "Cascadia Code"
style = "Italic"
```

#### `ui` {#font-ui}

Default: `normal`'s family and style · A table of `family` and `style`

The face of orzma's own interface: the workspace tab bar and its rename
field, the input method's preedit text, and the vi mode indicator. `family`
and `style` each default to those of `normal` and follow the same rules. With
the bundled font, orzma uses the bundled face closest to `style`.

```toml
[font]
ui = { family = "Inter", style = "Medium" }
```

### `[keyboard]`

#### `option_as_alt` {#keyboard-option_as_alt}

Default: `"right"` · `"none"`, `"left"`, `"right"`, or `"both"` · macOS only

Which Option key acts as `Alt`: it sends Meta to the program in the terminal
and runs `Alt` shortcuts. The other Option key types special characters, such
as `©` for Option+g; for shortcuts it still counts as `Alt` with a key that
types no character, such as an arrow, and together with `Ctrl` or `Cmd`. In vi
mode, an accent key pressed with the character-typing Option key, such as left
Option+e under the default, runs as its plain key.

A MacBook with a Japanese (JIS) keyboard has no right Option key; set
`"left"` or `"both"` there. On Windows and Linux every Alt key is `Alt` except
AltGr, which types characters, and this setting has no effect.

### `[mouse]`

Unknown keys in this table are ignored.

#### `lines_per_notch` {#mouse-lines_per_notch}

Default: `1` · Lines

How many lines one notch of wheel travel scrolls through the scrollback, or
how many arrow keys it sends on the alternate screen. Mouse reports to a
program ignore it.

#### `cells_per_notch` {#mouse-cells_per_notch}

Default: `0.3333` · Cells

How far the wheel or trackpad has to travel, in cells, to count as one notch.
The default makes one line of wheel travel three notches, which scroll three
lines. A smaller value scrolls faster. Mouse reports to a program ignore it:
they send one report per whole cell of travel.

#### `fine_modifier` {#mouse-fine_modifier}

Default: `"alt"` · `"alt"`, `"ctrl"`, `"shift"`, or `"none"`

The modifier that makes the wheel scroll `fine_lines` per notch instead of
`lines_per_notch`. `"none"` makes fine scrolling always active, and `"shift"`
has no effect on macOS.

#### `fine_lines` {#mouse-fine_lines}

Default: `1` · Lines

How many lines one notch scrolls while `fine_modifier` is held. At the
defaults it equals `lines_per_notch`, so the modifier changes nothing until
you raise `lines_per_notch`.

#### `max_protocol_events_per_frame` {#mouse-max_protocol_events_per_frame}

Default: `24` · A count

The most wheel events orzma sends to a program in one frame, on each axis:
mouse reports, and notches turned into arrow keys on the alternate screen.
Events past the limit are dropped.

#### `axis_lock_ratio` {#mouse-axis_lock_ratio}

Default: `0.9` · A number from 0 to 1

How strictly a trackpad swipe sticks to its main direction. The horizontal
part of a swipe is kept only when it makes up at least this share of the
motion; `0` turns the lock off, and `1` keeps horizontal motion only for a
purely horizontal swipe. A value outside 0 to 1 is clamped.

#### `double_click_timeout_ms` {#mouse-double_click_timeout_ms}

Default: `400` · Milliseconds

The longest pause between the clicks of a double or triple click.

#### `click_drift_px` {#mouse-click_drift_px}

Default: `8.0` · Logical pixels

How far the pointer may move between the clicks of a double or triple click.

#### `divider_grab_tolerance_px` {#mouse-divider_grab_tolerance_px}

Default: `4.0` · Logical pixels

How far from a pane divider, on each side, a press still grabs it for
resizing. It is never less than half a cell.

### `[inactive_pane]`

These keys set how panes other than the active pane are drawn. Unknown keys
in this table are ignored, and a number outside 0 to 1 is clamped.

#### `enabled` {#inactive_pane-enabled}

Default: `true` · `true` or `false`

Whether inactive panes are dimmed and tinted at all.

#### `dim` {#inactive_pane-dim}

Default: `1.0` · A number from 0 to 1

The brightness of an inactive pane: `1.0` leaves it as it is, and lower
values darken it.

#### `tint_color` {#inactive_pane-tint_color}

Default: `"#3a3b45"` · `"#RRGGBB"`

The color an inactive pane's background is blended toward. A value that is
not `#RRGGBB` keeps the default.

#### `tint` {#inactive_pane-tint}

Default: `0.85` · A number from 0 to 1

How far the background is blended toward `tint_color`: `0` keeps the real
background, and `1` replaces it. Text is not tinted.

#### `webview_dim` {#inactive_pane-webview_dim}

Default: `0.55` · A number from 0 to 1

The brightness of the web pages in an inactive pane.

#### `webview_desaturate` {#inactive_pane-webview_desaturate}

Default: `0.6` · A number from 0 to 1

How grey the web pages in an inactive pane turn: `0` keeps full color, and
`1` is fully grey.

### `[shortcuts]`

`[shortcuts]` binds orzma's own shortcuts; [Key Bindings](key-bindings.md)
describes the action keys, and
[Default Key Bindings](default-key-bindings.md) lists their defaults. The
table also has these keys:

#### `leader` {#shortcuts-leader}

Default: `"Cmd"` on macOS, `"Alt"` on Windows and Linux · A modifier or a chord

The key that starts a `<Leader>` binding: a modifier to tap, a chord to
press, or `""` to turn the leader off (see
[The leader key](key-bindings.md#the-leader-key)).

#### `leader-tap-timeout-ms` {#shortcuts-leader-tap-timeout-ms}

Default: `300` · Milliseconds

How long a tap of the leader modifier may last before it no longer counts.
`0` reverts to 300.

#### `repeat-time-ms` {#shortcuts-repeat-time-ms}

Default: `500` · Milliseconds

How long a `r:<Leader>` binding waits for its key again (see
[Repeatable bindings](key-bindings.md#repeatable-bindings-r)). `0` turns
repeating off for those bindings.

#### `direct-chords-over-webview` {#shortcuts-direct-chords-over-webview}

Default: `true` · `true` or `false`

Whether direct chords other than `copy` and `paste` run orzma's shortcuts
while a webview has focus. With `false`, the page gets them, except a direct
chord bound to `release-webview-focus`, which always runs (see
[Shortcuts while a webview has focus](key-bindings.md#shortcuts-while-a-webview-has-focus)).

### `[vi-mode]`

`[vi-mode]` binds the keys that work inside vi mode;
[Vi mode keys](key-bindings.md#vi-mode-keys-vi-mode) describes it.
