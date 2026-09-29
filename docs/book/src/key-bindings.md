# Key Bindings

orzma's own shortcuts live in the `[shortcuts]` table of the
[configuration file](configuration.md). The keys that work inside vi mode are
a separate table, described on the [Vi Mode](vi-mode.md) page.

## The leader key

Most actions are bound to the *leader* followed by one more key, written
`<Leader>` in the configuration. By default the leader is a tap of a
modifier: `Cmd` on macOS and `Alt` on Windows and Linux. Press and release the modifier
with no other key or mouse button in between, then press the action's key.
Holding the modifier works as usual, so `Alt+h` still reaches the shell as a
meta-prefixed key (on macOS, with the Option key that `option_as_alt` makes
Alt).

After a tap, the next keystroke runs a `<Leader>` action when one matches.
Otherwise it runs the key's direct chord, if it has one, and is swallowed if it
has none. The leader does not time out while it waits, even if you switch to
another window and back. Switching windows before you release the modifier
cancels the tap.

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
key instead — with the stock bindings, `Shift+H` and `Shift+L` jump to the top
and bottom visible line, and `Shift+J` and `Shift+K` do nothing.

Earlier releases wrote a repeatable leader binding as `<Leader:r>x`. That
spelling is not accepted: a configuration that still uses it is ignored as a
whole (see [Validation](configuration.md#validation)), so rewrite `<Leader:r>x`
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

## Actions

The `Default` column lists the macOS value; see "Platform defaults" above for
the seven that differ elsewhere.

| Action | Default | What it does |
| --- | --- | --- |
| `paste` | `Cmd+V` | Paste from the system clipboard. |
| `copy` | `Cmd+C` | Copy the focused terminal's selection to the system clipboard, then dismiss the selection. |
| `increase-font-size` | `r:Cmd+Plus` | Step the terminal font size up. |
| `decrease-font-size` | `r:Cmd+-` | Step the terminal font size down. |
| `reset-font-size` | `Cmd+0` | Return the terminal font size to `[font] size`. |
| `release-webview-focus` | `<Leader>u` | Return keyboard focus from a focused webview to the terminal. |
| `quit` | `Cmd+Q` | Quit orzma. |
| `enter-vi-mode` | `<Leader>s` | Enter vi mode. |
| `select-left-pane` | `<Leader>h` | Focus the pane to the left. |
| `select-down-pane` | `<Leader>j` | Focus the pane below. |
| `select-up-pane` | `<Leader>k` | Focus the pane above. |
| `select-right-pane` | `<Leader>l` | Focus the pane to the right. |
| `resize-left-pane` | `r:<Leader>Shift+H` | Move a divider of the active pane 5 cells left, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |
| `resize-down-pane` | `r:<Leader>Shift+J` | Move a divider of the active pane 5 cells down, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |
| `resize-up-pane` | `r:<Leader>Shift+K` | Move a divider of the active pane 5 cells up, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |
| `resize-right-pane` | `r:<Leader>Shift+L` | Move a divider of the active pane 5 cells right, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |
| `split-vertical-pane` | `<Leader>i` | Split the active pane side by side (vertical divider); the new pane becomes active. |
| `split-horizontal-pane` | `<Leader>o` | Split the active pane stacked (horizontal divider); the new pane becomes active. |
| `kill-pane` | `<Leader>p` | Kill the active pane; its shell is terminated. |
| `new-workspace` | `<Leader>c` | Open a new workspace after the last one and show it. |
| `close-workspace` | `<Leader>Shift+X` | Close the workspace on screen and end every shell in it. |
| `next-workspace` | `<Leader>]` | Show the workspace to the right, wrapping around. |
| `previous-workspace` | `<Leader>[` | Show the workspace to the left, wrapping around. |
| `select-workspace-1` … `select-workspace-9` | `<Leader>1` … `<Leader>9` | Show the first … ninth workspace. |
| `rename-workspace` | `<Leader>r` | Rename the workspace on screen. |

The window actions orzma 0.1.0 accepted (`new-window`, `next-window`,
`select-window-0` and the rest, `rename-window`) and `zoom-pane` have been
removed; workspaces replace the window actions under new names (see
[Workspaces](multiplexer.md#workspaces)). A configuration that still sets one
of the old keys is ignored as a whole (see
[Validation](configuration.md#validation)), so delete those lines when you
upgrade.

## Conflicts and turning the leader off

Three consequences of the stock `<Leader>` defaults worth knowing:

- **Rebinding a `<Leader>` chord that a stock default already uses** (e.g.
  `split-vertical-pane = "<Leader>h"`, which collides with the default
  `select-left-pane = "<Leader>h"`) is a startup validation error naming both
  actions. Unbind the stock default explicitly (`select-left-pane = ""`) or
  pick a free chord.
- **The workspace actions brought new stock defaults** — `<Leader>c`,
  `<Leader>r`, `<Leader>[`, `<Leader>]`, `<Leader>1` … `<Leader>9`, and
  `<Leader>Shift+X`. If your configuration already binds one of these chords
  to another action, orzma does not start after the upgrade (see
  [Validation](configuration.md#validation)). Rebind that action to a free
  chord, or unbind the workspace action that takes the chord, e.g.
  `rename-workspace = ""`.
- **`leader = ""` disables every `<Leader>`-bound action at once** — with the
  stock defaults that includes all 27 leader-bound actions above, silently
  (a warning is logged, but startup succeeds). If you disable the leader,
  rebind the actions you need to direct chords, e.g.
  `split-vertical-pane = "Ctrl+Shift+I"`.

## Shortcuts while a webview has focus

Once you click into a webview, keys go to the page, but orzma's own shortcuts
still come first:

- `<Leader>` bindings and `release-webview-focus` always run.
- Other direct chords, such as `Cmd+Plus` or a pane action bound to a chord,
  run while `direct-chords-over-webview` is `true`, the default. Set it to
  `false` to let the page have them.
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

The stock `[shortcuts]` table, with the macOS defaults:

```toml
[shortcuts]
# NOTE: the values in this block are the macOS defaults. Seven of them differ on
# Windows and Linux — see "Platform defaults" above for the other table.
# The leader for "<Leader>..." bindings. Either a full chord ("Ctrl+A": press
# the chord, then the next key) OR a bare modifier to TAP ("Cmd"/"Ctrl"/"Alt":
# tap the modifier with no other key, then the next key). Defaults to "Cmd" on
# macOS and "Alt" elsewhere, and is active only when at least one action is
# bound to "<Leader>..." — the stock defaults below already bind 27
# actions to "<Leader>...", so the tap leader is armed out of the box.
# Set "" to disable it. "Shift" is not allowed as a tap.
leader = "Cmd"
# Modifier-tap window (ms): a press+release within this time, with no intervening
# key or mouse press, counts as a tap. Default 300; 0 reverts to 300.
leader-tap-timeout-ms = 300
# Repeat window (ms) for "r:<Leader>..." bindings: after such a binding fires,
# pressing a repeat-marked key again within this window re-fires the action
# without the leader. Each fire re-arms the window. Default 500; 0 disables
# repeat entirely.
repeat-time-ms = 500
# Direct chords ("Cmd+Plus", ...) run even while a webview has keyboard focus.
# "<Leader>..." bindings and release-webview-focus always do, and direct copy
# and paste chords never do. Set false to hand the other direct chords to the
# page.
direct-chords-over-webview = true

# Each action takes ONE value: a direct chord ("Cmd+V"), a leader-scoped
# chord ("<Leader>s" = leader then s), either one preceded by "r:" to make
# it repeatable ("r:<Leader>s" re-fires within repeat-time-ms, "r:Cmd+Plus"
# re-fires while held), or "" to unbind. A direct chord without "r:" fires
# once per press. Rebinding to a chord already used by another action is a
# startup validation error. A direct chord and a "<Leader>"-prefixed chord
# with the same key never collide.

# --- existing actions ---
paste                 = "Cmd+V"        # Standard terminal paste; set paste = "<Leader>v" for a leader binding.
copy                  = "Cmd+C"        # Copy the focused terminal's selection to the system clipboard.
release-webview-focus = "<Leader>u"
quit                  = "Cmd+Q"        # Unbound by default off macOS, where the window manager closes the window.
enter-vi-mode         = "<Leader>s"    # Enters vi mode.

# --- pane actions ---
select-left-pane      = "<Leader>h"    # select-pane -L
select-down-pane      = "<Leader>j"    # select-pane -D
select-up-pane        = "<Leader>k"    # select-pane -U
select-right-pane     = "<Leader>l"    # select-pane -R
split-vertical-pane   = "<Leader>i"    # split-window -h (side-by-side)
split-horizontal-pane = "<Leader>o"    # split-window -v (stacked)
kill-pane             = "<Leader>p"    # kill-pane
resize-left-pane      = "r:<Leader>Shift+H"  # resize-pane -L 5 (repeatable)
resize-down-pane      = "r:<Leader>Shift+J"  # resize-pane -D 5 (repeatable)
resize-up-pane        = "r:<Leader>Shift+K"  # resize-pane -U 5 (repeatable)
resize-right-pane     = "r:<Leader>Shift+L"  # resize-pane -R 5 (repeatable)

# --- workspace actions ---
new-workspace         = "<Leader>c"        # new-window
close-workspace       = "<Leader>Shift+X"  # kill-window
next-workspace        = "<Leader>]"        # next-window
previous-workspace    = "<Leader>["        # previous-window
select-workspace-1    = "<Leader>1"
select-workspace-2    = "<Leader>2"
select-workspace-3    = "<Leader>3"
select-workspace-4    = "<Leader>4"
select-workspace-5    = "<Leader>5"
select-workspace-6    = "<Leader>6"
select-workspace-7    = "<Leader>7"
select-workspace-8    = "<Leader>8"
select-workspace-9    = "<Leader>9"
rename-workspace      = "<Leader>r"        # rename-window

# --- zoom actions ---
increase-font-size    = "r:Cmd+Plus"   # r:Ctrl+Plus off macOS
decrease-font-size    = "r:Cmd+-"      # r:Ctrl+- off macOS
reset-font-size       = "Cmd+0"      # Ctrl+0 off macOS
```
