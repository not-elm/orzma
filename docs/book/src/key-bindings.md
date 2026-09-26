# Key Bindings

orzma's own shortcuts live in the `[shortcuts]` table of the
[configuration file](configuration.md). The keys that work inside vi mode are
a separate table, described on the [Vi Mode](vi-mode.md) page.

## The leader key

Most actions are bound to the *leader* followed by one more key, written
`<Leader>` in the configuration. By default the leader is a tap of a
modifier: `Cmd` on macOS and `Alt` on Windows. Press and release the modifier
with no other key or mouse button in between, then press the action's key.
Holding the modifier works as usual, so `Alt+h` still reaches the shell as a
meta-prefixed key.

After a tap, the next keystroke either runs a `<Leader>` action or is
swallowed if no action matches; the leader does not time out while it waits,
even if you switch to another window and back. Switching windows before you
release the modifier cancels the tap.

- `leader` sets the leader: a modifier to tap (`"Cmd"`, `"Ctrl"`, or
  `"Alt"`; `"Shift"` is not allowed), a chord such as `"Ctrl+A"` (press the
  chord, then the action's key), or `""` to turn the leader off.
- `leader-tap-timeout-ms` is how long a tap may last before it no longer
  counts, 300 ms by default; `0` reverts to 300.
- `repeat-time-ms` is the repeat window of `<Leader:r>` bindings (below),
  500 ms by default; `0` turns repeating off.

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

## Repeatable bindings (`<Leader:r>`)

Binding an action with `<Leader:r>` instead of `<Leader>` makes it repeatable:
after the binding fires, pressing any repeat-marked key
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

Binding `paste` to `Ctrl+V` does take that key away from the program running in
the terminal, so readline's quoted-insert and vim's visual-block mode no longer
see it. Set `paste = "Ctrl+Shift+V"` to give it back.

How `Ctrl+C` copies or interrupts, and how `Shift` works with programs that
track the mouse, are described in
[Terminal Features](terminal-features.md#selection-and-clipboard).

## Actions

The `Default` column lists the macOS value; see "Platform defaults" above for
the seven that differ elsewhere.

| Action | Default | What it does |
| --- | --- | --- |
| `paste` | `Cmd+V` | Paste from the system clipboard. |
| `copy` | `Cmd+C` | Copy the focused terminal's selection to the system clipboard, then dismiss the selection. |
| `increase-font-size` | `Cmd+Plus` | Step the terminal font size up. |
| `decrease-font-size` | `Cmd+-` | Step the terminal font size down. |
| `reset-font-size` | `Cmd+0` | Return the terminal font size to `[font] size`. |
| `release-webview-focus` | `<Leader>u` | Return keyboard focus from a focused webview to the terminal. |
| `quit` | `Cmd+Q` | Quit orzma. |
| `enter-vi-mode` | `<Leader>s` | Enter vi mode. |
| `select-left-pane` | `<Leader>h` | Focus the pane to the left. |
| `select-down-pane` | `<Leader>j` | Focus the pane below. |
| `select-up-pane` | `<Leader>k` | Focus the pane above. |
| `select-right-pane` | `<Leader>l` | Focus the pane to the right. |
| `resize-left-pane` | `<Leader:r>Shift+H` | Move a divider of the active pane 5 cells left, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |
| `resize-down-pane` | `<Leader:r>Shift+J` | Move a divider of the active pane 5 cells down, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |
| `resize-up-pane` | `<Leader:r>Shift+K` | Move a divider of the active pane 5 cells up, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |
| `resize-right-pane` | `<Leader:r>Shift+L` | Move a divider of the active pane 5 cells right, repeatable (see [Resizing panes](multiplexer.md#resizing-panes)). |
| `split-vertical-pane` | `<Leader>i` | Split the active pane side by side (vertical divider); the new pane becomes active. |
| `split-horizontal-pane` | `<Leader>o` | Split the active pane stacked (horizontal divider); the new pane becomes active. |
| `kill-pane` | `<Leader>p` | Kill the active pane; its shell is terminated. |
| `zoom-pane` | `<Leader>z` | Toggle zoom on the active pane (not implemented yet). |
| `new-window` | `<Leader>c` | Open a new window (not implemented yet). |
| `kill-window` | `<Leader>Shift+X` | Kill the active window, after a confirm prompt (not implemented yet). |
| `next-window` | `<Leader>]` | Switch to the next window (not implemented yet). |
| `previous-window` | `<Leader>[` | Switch to the previous window (not implemented yet). |
| `select-window-0` | `<Leader>0` | Switch to the window at index 0 (not implemented yet). |
| `select-window-1` | `<Leader>1` | Switch to the window at index 1 (not implemented yet). |
| `select-window-2` | `<Leader>2` | Switch to the window at index 2 (not implemented yet). |
| `select-window-3` | `<Leader>3` | Switch to the window at index 3 (not implemented yet). |
| `select-window-4` | `<Leader>4` | Switch to the window at index 4 (not implemented yet). |
| `select-window-5` | `<Leader>5` | Switch to the window at index 5 (not implemented yet). |
| `select-window-6` | `<Leader>6` | Switch to the window at index 6 (not implemented yet). |
| `select-window-7` | `<Leader>7` | Switch to the window at index 7 (not implemented yet). |
| `select-window-8` | `<Leader>8` | Switch to the window at index 8 (not implemented yet). |
| `select-window-9` | `<Leader>9` | Switch to the window at index 9 (not implemented yet). |
| `rename-window` | `<Leader>r` | Open the rename prompt for the active window (not implemented yet). |

> [!NOTE]
> The actions marked "not implemented yet" are accepted and validated at
> startup, but pressing them does nothing, whether they are bound directly or
> behind the leader.

## Conflicts and turning the leader off

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
  `split-vertical-pane = "Ctrl+Shift+I"`.

## The `+` and `-` keys

`Ctrl++` is not a valid value: a chord is split on `+`, so write `Ctrl+Plus`.
A `Plus` binding also fires with Shift held, since `+` is Shift+`=` on a US
layout. Key bindings match physical key positions, so on a non-US layout the
`Plus` and `-` positions may not be where the labels are. The numeric keypad's
`+` and `-` are not bindable. `Plus` resolves to the physical position of the
`=` key on a US layout, and fires whether or not Shift is held — including
when it is the leader. On a layout with a dedicated `+` key, such as German,
that position is a different key, so bind the key you actually want by name
instead.

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
zoom-pane             = "<Leader>z"    # resize-pane -Z
resize-left-pane      = "<Leader:r>Shift+H"  # resize-pane -L 5 (repeatable)
resize-down-pane      = "<Leader:r>Shift+J"  # resize-pane -D 5 (repeatable)
resize-up-pane        = "<Leader:r>Shift+K"  # resize-pane -U 5 (repeatable)
resize-right-pane     = "<Leader:r>Shift+L"  # resize-pane -R 5 (repeatable)

# --- zoom actions ---
increase-font-size    = "Cmd+Plus"   # Ctrl+Plus off macOS
decrease-font-size    = "Cmd+-"      # Ctrl+- off macOS
reset-font-size       = "Cmd+0"      # Ctrl+0 off macOS

# --- window actions (not implemented yet) ---
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

# --- rename action (not implemented yet) ---
rename-window         = "<Leader>r"        # opens the rename prompt for the active window
```
