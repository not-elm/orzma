# Multiplexer

orzma can split its window into panes, each running its own shell. The panes
are part of orzma itself, so there is no separate multiplexer to start.

| Default keys | Action |
| --- | --- |
| Leader, then `i` | Split the active pane side by side. |
| Leader, then `o` | Split the active pane top and bottom. |
| Leader, then `h` / `j` / `k` / `l` | Make the pane on the left / below / above / on the right active. |
| Leader, then `p` | Close the active pane and end its shell. |
| Leader, then `Shift+H` / `Shift+J` / `Shift+K` / `Shift+L` | Move a divider of the active pane 5 cells left / down / up / right. |

The leader is a tap of `Cmd` on macOS or `Alt` on Windows: press and release it
on its own, then press the next key. See [Key Bindings](key-bindings.md) to
change the leader or these keys.

A new pane becomes the active pane. You can also click a pane to make it
active, and drag the border between two panes to resize them. When the last
pane closes, orzma quits.

## Resizing panes

The resize keys move one divider of the active pane 5 cells in the key's
direction, picking it the way tmux's `resize-pane` does. They are repeatable:
press the key again within `repeat-time-ms` (500 ms by default), without the
leader, to keep moving the divider (see
[Repeatable bindings](key-bindings.md#repeatable-bindings-leaderr)).

Left and right look at the row of side-by-side panes the active pane belongs
to (up and down at its column of stacked panes): the divider after the pane
moves when there is one, and otherwise the divider before it. So left and
right move the active pane's right border unless the pane is the last one in
its row. Panes nested inside the area that grows or shrinks keep their
proportions; when the active pane is one of them, it changes by only its share
of the 5 cells (possibly none) and its other border can move as well.

A divider stops once the side it shrinks reaches 4 columns or 2 rows per pane
(less when the area the divider splits is too small to give both sides that
much), so a small pane nested on that side can still end up narrower. A
divider never moves against the key, and a key with no divider to move on its
axis does nothing. In vi mode each step needs the leader again.

## Working directory of a new pane

A pane made by a split starts in the working directory of the pane it was split
from:

- On macOS, orzma asks the system for the directory of the pane's foreground
  program. If the system does not report one, orzma uses the directory the
  shell last reported with the OSC 7 or OSC 9;9 escape sequence, and then the
  directory the pane started in.
- On Windows, the directory the shell last reported comes first. orzma makes
  PowerShell (`pwsh` and `powershell`) and `cmd` report it automatically; set
  `shell_integration = false` in the [`[orzma]`](configuration.md#orzma) table
  to turn this off.

orzma uses the directory only if it still exists; otherwise, and for the first
pane, the new pane starts in your home directory.

## Inactive panes

Panes that do not have focus are drawn with a tint. Change or turn off the
effect in the [`[inactive_pane]`](configuration.md#inactive_pane) table.

> [!NOTE]
> Windows (groups of panes you switch between) and zooming a pane are not
> implemented yet. Their actions, such as `new-window` and `zoom-pane`, are
> accepted in the configuration but do nothing.
