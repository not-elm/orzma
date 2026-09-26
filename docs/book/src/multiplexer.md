# Multiplexer

orzma can split its window into panes, each running its own shell. The panes
are part of orzma itself, so there is no separate multiplexer to start.

| Default keys | Action |
| --- | --- |
| Leader, then `i` | Split the active pane side by side. |
| Leader, then `o` | Split the active pane top and bottom. |
| Leader, then `h` / `j` / `k` / `l` | Make the pane on the left / below / above / on the right active. |
| Leader, then `p` | Close the active pane and end its shell. |

The leader is a tap of `Cmd` on macOS or `Alt` on Windows: press and release it
on its own, then press the next key. See [Key Bindings](key-bindings.md) to
change the leader or these keys.

A new pane becomes the active pane. You can also click a pane to make it
active, and drag the border between two panes to resize them. When the last
pane closes, orzma quits.

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
> Windows (groups of panes you switch between), zooming a pane, and resizing
> panes from the keyboard are not implemented yet. Their actions, such as
> `new-window`, `zoom-pane`, and `resize-left-pane`, are accepted in the
> configuration but do nothing.
