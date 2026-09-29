# Multiplexer

orzma can split its window into panes, each running its own shell, and keep
several such layouts as workspaces, listed as tabs across the top of the
window. The panes are part of orzma itself, so there is no separate
multiplexer to start.

| Default keys | Action |
| --- | --- |
| Leader, then `i` | Split the active pane side by side. |
| Leader, then `o` | Split the active pane top and bottom. |
| Leader, then `h` / `j` / `k` / `l` | Make the pane on the left / below / above / on the right active. |
| Leader, then `p` | Close the active pane and end its shell. |
| Leader, then `Shift+H` / `Shift+J` / `Shift+K` / `Shift+L` | Move a divider of the active pane 5 cells left / down / up / right. |

The leader is a tap of `Cmd` on macOS or `Alt` on Windows and Linux: press and
release it on its own, then press the next key. See
[Key Bindings](key-bindings.md) to change the leader or these keys.

A new pane becomes the active pane. You can also click a pane to make it
active, and drag the border between two panes to resize them. When the last
pane of a workspace closes, the workspace closes; when the last workspace
closes, orzma quits.

## Workspaces

A workspace is one layout of panes. The tab bar across the top of the window
lists every workspace; the highlighted tab is the one on screen. The shells in
the other workspaces keep running, and pages shown in them keep their state.

| Default keys | Action |
| --- | --- |
| Leader, then `c` | Open a new workspace after the last one and show it. |
| Leader, then `Shift+X` | Close the workspace on screen and end every shell in it. |
| Leader, then `]` / `[` | Show the workspace to the right / left (wrapping around). |
| Leader, then `1` … `9` | Show the first … ninth workspace. |
| Leader, then `r` | Rename the workspace on screen. |

Click a tab to show its workspace, click its `×` to close it, and click `+` to
open a new one. When the tabs do not fit in the window, turn the mouse wheel
over the tab bar to scroll through them. A workspace you have not named is
called `Workspace n`, where `n` is its position in the tab bar. A new workspace
starts in the working directory of the active pane, like a split.

Double-click a tab, or press the leader and then `r`, to rename its workspace
in place. Enter or a click anywhere else keeps the new name, and Esc keeps the
old one. Leave the field empty to go back to `Workspace n`. Drag a tab to move
its workspace; the numbers of unnamed workspaces follow their new positions.

## Resizing panes

The resize keys move one divider of the active pane 5 cells in the key's
direction, picking it the way tmux's `resize-pane` does. They are repeatable:
press the key again within `repeat-time-ms` (500 ms by default), without the
leader, to keep moving the divider (see
[Repeatable bindings](key-bindings.md#repeatable-bindings-r)).

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

- On macOS and Linux, orzma asks the system for the directory of the pane's
  foreground program. If the system does not report one, orzma uses the directory the
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
