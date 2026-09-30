# Key Bindings

orzma's own shortcuts live in the `[shortcuts]` table of the
[configuration file](configuration.md). The keys that work inside vi mode are
a separate table, described on the [Vi Mode](vi-mode.md) page.

## The leader key

Most stock shortcuts are direct `Alt` chords, such as `Alt+h` (see
[Actions](#actions)). The *leader* is a second way to bind an action: the
leader followed by one more key, written `<Leader>` in the configuration. By
default only `release-webview-focus` (`<Leader>u`) uses it. The leader is a tap
of a modifier: `Cmd` on macOS and `Alt` on Windows and Linux. Press and release
the modifier with no other key or mouse button in between, then press the
action's key.

After a tap, the next keystroke runs a `<Leader>` action when one matches.
Otherwise it is handled as if no leader had been tapped when it has a direct
chord, and is swallowed when it has none. The leader does not time out while
it waits, even if you switch to another window and back. Switching windows
before you release the modifier cancels the tap.

- `leader` sets the leader: a modifier to tap (`"Cmd"`, `"Ctrl"`, or
  `"Alt"`; `"Shift"` is not allowed), a chord such as `"Ctrl+A"` (press the
  chord, then the action's key), or `""` to turn the leader off.
- `leader-tap-timeout-ms` is how long a tap may last before it no longer
  counts, 300 ms by default; `0` reverts to 300.
- `repeat-time-ms` is the repeat window of `r:<Leader>` bindings (below),
  500 ms by default; `0` turns repeating off for them. It does not affect
  direct chords marked `r:`.

## Chord syntax

A chord is zero or more modifiers followed by exactly one key, joined with `+`.

- **Modifiers** (case-insensitive): `Cmd` (also `Command` / `Meta` / `Super`),
  `Ctrl`, `Shift`, `Alt` (also `Opt` / `Option`).
- **Keys**: a letter or a digit (letters are case-insensitive), `[`, `]`, `-`,
  `=`, or a named key: `Escape` `Space` `Enter` `Tab` `Backspace` `ArrowUp`
  `ArrowDown` `ArrowLeft` `ArrowRight` `Plus`. Any other character is accepted
  but never fires.
- For the `+` key itself, use the token `Plus` (e.g. `Cmd+Plus`).

Examples: `Cmd+Shift+Q`, `Ctrl+Alt+ArrowLeft`, `Cmd+Plus`.

Invalid chords — an empty token (`Cmd+`), an unknown named key (`Cmd+F12`), a
duplicated modifier (`Cmd+Meta+S`), or more than one key (`Cmd+S+T`) — make
orzma ignore the whole file and start with the defaults (see
[Validation](configuration.md#validation)).

## Repeatable bindings (`r:`)

Put `r:` in front of a binding to make it repeatable. It works on both kinds of
binding:

- **A direct chord** such as `r:Alt+Shift+H` keeps firing while you hold it,
  at your system's key repeat rate. A direct chord without `r:` fires once per
  press, however long you hold it.
- **A leader binding** such as `r:<Leader>Shift+H` re-fires without the leader:
  after it fires, pressing any repeat-marked key again within `repeat-time-ms`
  (default 500) runs its action again, and each fire re-arms the window.
  Holding the key down keeps firing. Any other key — including keys bound with
  plain `<Leader>` — closes the window immediately and is handled normally (it
  is never swallowed). Pressing the leader inside the window starts a fresh
  leader sequence.

The stock resize bindings, `increase-font-size`, and `decrease-font-size` carry
`r:`; no other stock binding does.

Caveat: with a letter key (say `r:<Leader>h`), typing that same letter into the
shell within the window re-fires the action instead of reaching the terminal.
If that bites, set `repeat-time-ms = 0` or drop the `r:` from that binding.

In vi mode a repeatable leader binding fires only on the key pressed right
after the leader: the window closes on the next key event, and holding the key
does not keep firing. A second press or an auto-repeat is read as a `[vi-mode]`
key instead. A repeatable direct chord, such as the stock `r:Alt+Shift+H`, runs
before the `[vi-mode]` keys and keeps firing while held.

Earlier releases wrote a repeatable leader binding as `<Leader:r>x`. That
spelling is not accepted: a configuration that still uses it is ignored as a
whole — orzma warns and starts with defaults (see
[Validation](configuration.md#validation)), so rewrite `<Leader:r>x`
as `r:<Leader>x` when you upgrade. A direct chord you bound yourself also stops
repeating while held until you add `r:` to it.

## Platform defaults

Seven defaults differ by platform, because macOS has a `Cmd` key and the other
platforms do not. Every other action below is the same everywhere.

| Action | Default (macOS) | Default (Windows / Linux) |
| --- | --- | --- |
| `leader` | `Cmd` (tap) | `Alt` (tap) |
| `paste` | `Cmd+V` | `Ctrl+V` |
| `copy` | `Cmd+C` | `Ctrl+C` |
| `increase-font-size` | `r:Cmd+Plus` | `r:Ctrl+Plus` |
| `decrease-font-size` | `r:Cmd+-` | `r:Ctrl+-` |
| `reset-font-size` | `Cmd+0` | `Ctrl+0` |
| `quit` | `Cmd+Q` | unbound |

`quit` ships unbound off macOS because the window manager's own close
shortcut (`Alt+F4` on Windows) already exits orzma. Bind it explicitly if you
want a second way out.

Binding `paste` to `Ctrl+V` does take that key away from the program running in
the terminal, so readline's quoted-insert and vim's visual-block mode no longer
see it. Set `paste = "Ctrl+Shift+V"` to give it back.

A copy chord that uses `Ctrl` alone, such as the stock `Ctrl+C`, copies only
while text is selected; with nothing selected, it reaches the program as usual,
so `Ctrl+C` still interrupts. A copy chord with any other modifier, such as the
macOS `Cmd+C`, always copies.

## `Alt` chords, the shell, and the Option key

The stock pane, workspace, and vi-mode shortcuts are `Alt` chords, so those
keys no longer reach the program in the terminal as meta-prefixed keys:
readline's `Alt+c` (capitalize word), `Alt+r` (revert line), and `Alt+1` …
`Alt+9` (numeric argument), for example. Unbind or rebind a shortcut to give
its key back to the shell. `Alt` chords that orzma does not bind, such as
`Alt+b` and `Alt+f`, still reach the shell.

- **macOS:** the Option key that `option_as_alt` makes Alt runs the `Alt`
  chords — the right Option key by default (see
  [Configuration](configuration.md#keyboard)). The other Option key types
  special characters, so left `Option+i` starts an accent instead of splitting
  the pane; it still counts as `Alt` with an arrow or another key that types
  no character, and together with `Ctrl` or `Cmd`. A MacBook with a Japanese
  (JIS) keyboard has no right Option key; set `option_as_alt = "left"` or
  `"both"` there. Under `option_as_alt = "none"`, the default of earlier
  releases, neither Option key runs the stock `Alt` chords, so remove that line
  from a configuration that still sets it.
- **Windows and Linux:** on a keyboard layout with AltGr, such as German, the
  right Alt key is AltGr and types characters; use the left Alt key for the
  shortcuts.

## Actions

Each action takes one binding: a direct chord such as `Alt+h`, a `<Leader>`
binding such as `<Leader>u`, either of them with `r:` in front, or `""` to
unbind it. An action you leave out keeps its default. The `Default` column
lists the macOS value; see "Platform defaults" above for the seven that differ
elsewhere.

| Action | Default | What it does |
| --- | --- | --- |
| `paste` | `Cmd+V` | Paste from the system clipboard. |
| `copy` | `Cmd+C` | Copy the focused terminal's selection to the system clipboard, then dismiss the selection. |
| `increase-font-size` | `r:Cmd+Plus` | Step the terminal font size up. |
| `decrease-font-size` | `r:Cmd+-` | Step the terminal font size down. |
| `reset-font-size` | `Cmd+0` | Return the terminal font size to `[font] size`. |
| `release-webview-focus` | `<Leader>u` | Return keyboard focus from a focused webview to the terminal. |
| `quit` | `Cmd+Q` | Quit orzma. |
| `enter-vi-mode` | `Alt+s` | Enter vi mode. |

### Pane actions

<!-- ANCHOR: pane-actions -->

| Action | Default | What it does |
| --- | --- | --- |
| `split-vertical-pane` | `Alt+i` | Split the active pane side by side (vertical divider); the new pane becomes active. |
| `split-horizontal-pane` | `Alt+o` | Split the active pane top and bottom (horizontal divider); the new pane becomes active. |
| `select-left-pane` | `Alt+h` | Make the pane to the left active. |
| `select-down-pane` | `Alt+j` | Make the pane below active. |
| `select-up-pane` | `Alt+k` | Make the pane above active. |
| `select-right-pane` | `Alt+l` | Make the pane to the right active. |
| `kill-pane` | `Alt+p` | Close the active pane and end its shell. |
| `resize-left-pane` | `r:Alt+Shift+H` | Move a divider of the active pane 5 cells left, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |
| `resize-down-pane` | `r:Alt+Shift+J` | Move a divider of the active pane 5 cells down, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |
| `resize-up-pane` | `r:Alt+Shift+K` | Move a divider of the active pane 5 cells up, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |
| `resize-right-pane` | `r:Alt+Shift+L` | Move a divider of the active pane 5 cells right, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |

<!-- ANCHOR_END: pane-actions -->

### Workspace actions

<!-- ANCHOR: workspace-actions -->

| Action | Default | What it does |
| --- | --- | --- |
| `new-workspace` | `Alt+c` | Open a new workspace after the last one and show it. |
| `close-workspace` | `Alt+Shift+X` | Close the workspace on screen and end every shell in it. |
| `next-workspace` | `Alt+]` | Show the workspace to the right, wrapping around. |
| `previous-workspace` | `Alt+[` | Show the workspace to the left, wrapping around. |
| `select-workspace-1` … `select-workspace-9` | `Alt+1` … `Alt+9` | Show the first … ninth workspace. |
| `rename-workspace` | `Alt+r` | Rename the workspace on screen. |

<!-- ANCHOR_END: workspace-actions -->

The window actions orzma 0.1.0 accepted (`new-window`, `next-window`,
`select-window-0` and the rest, `rename-window`) and `zoom-pane` have been
removed; workspaces replace the window actions under new names (see
[Workspaces](multiplexer.md#workspaces)). A configuration that still sets one
of the old keys is ignored as a whole (see
[Validation](configuration.md#validation)), so delete those lines when you
upgrade.

## Conflicts and turning the leader off

Three consequences of the stock defaults worth knowing:

- **Rebinding a chord that a stock default already uses** (e.g.
  `split-vertical-pane = "Alt+h"`, which collides with the default
  `select-left-pane = "Alt+h"`) is a startup validation error naming both
  actions. Unbind the stock default explicitly (`select-left-pane = ""`) or
  pick a free chord.
- **The stock pane, workspace, and vi-mode shortcuts are `Alt` chords** (see
  [Actions](#actions)). If your configuration binds one of those chords to
  another action, or uses one of them as a chord `leader`, orzma does not
  start (see [Validation](configuration.md#validation)). Rebind that action to a free
  chord, or unbind the stock action that takes the chord, e.g.
  `rename-workspace = ""`. Binding a chord to the action that already has it by
  default is not a conflict. A direct chord and a `<Leader>` binding never
  collide, even on the same key, such as `s` and `<Leader>s`.
- **`leader = ""` disables every `<Leader>`-bound action at once** — with the
  stock defaults that is only `release-webview-focus`, silently (a warning is
  logged, but startup succeeds). If you disable the leader, rebind it to a
  direct chord, e.g. `release-webview-focus = "Ctrl+Shift+U"`.

## Shortcuts while a webview has focus

Once you click into a webview, keys go to the page, but orzma's own shortcuts
still come first:

- `<Leader>` bindings and `release-webview-focus` always run.
- Other direct chords, such as `Cmd+Plus` or the stock `Alt` pane and
  workspace chords, run while `direct-chords-over-webview` is `true`, the
  default. Set it to `false` to let the page have them; `release-webview-focus`
  (`<Leader>u`) still takes the keyboard back.
- `copy` and `paste` bound to direct chords never run while a webview has
  focus, so `Cmd+C` and `Cmd+V` copy and paste inside the page.

A chord that runs an orzma shortcut never reaches the page, and it runs even
when the page's program lists it as a forward key.

## The `+` and `-` keys

`Ctrl++` is not a valid value: a chord is split on `+`, so write `Ctrl+Plus`.
A `Plus` binding also fires with Shift held, since `+` is Shift+`=` on a US
layout. Key bindings match physical key positions, so on a non-US layout the
`Plus` and `-` positions may not be where the labels are. The numeric keypad's
`+` and `-` are not bindable. `Plus` resolves to the physical position of the
`=` key on a US layout, and fires whether or not Shift is held — including
when it is the leader. On a layout with a dedicated `+` key, such as German,
that position is a different key, so bind the key you actually want by name
instead. `[` and `]` name physical key positions of a US layout; on a JIS
keyboard they are the keys labelled `@` and `[`.

## Example

A `[shortcuts]` table that changes a few defaults and keeps the rest:

```toml
[shortcuts]
# Tap Ctrl to start a <Leader> binding.
leader = "Ctrl"
# Split with the leader, giving Alt+i and Alt+o back to the shell.
split-vertical-pane   = "<Leader>i"
split-horizontal-pane = "<Leader>o"
# Give Alt+r back to the shell.
rename-workspace      = ""
# Keep resizing while Shift+H is held or pressed again within repeat-time-ms.
resize-left-pane      = "r:<Leader>Shift+H"
```
