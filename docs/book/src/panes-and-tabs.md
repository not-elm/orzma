# Panes and Tabs

orzma can split its window into panes, each running its own shell, and keep
several such layouts as tabs across the top of the window. The panes are part
of orzma itself, so there is no separate multiplexer to start.

{{#include default-key-bindings.md:pane-actions}}

`Alt` is the right Option key on macOS by default, and either Alt key on
Windows and Linux (the left one on a keyboard with AltGr). See
[Key Bindings](key-bindings.md) to change these keys, and
[`Alt` chords, the shell, and the Option key](key-bindings.md#alt-chords-the-shell-and-the-option-key)
for what they take away from the shell.

A new pane becomes the active pane. You can also click a pane to make it
active, and drag the border between two panes to resize them. When the last
pane of a tab closes, the tab closes; when the last tab closes, orzma quits.

## Tabs

A tab is one layout of panes. The tab bar across the top of the window lists
every tab; the highlighted tab is the one on screen. The shells in the other
tabs keep running, and pages shown in them keep their state.

{{#include default-key-bindings.md:tab-actions}}

Click a tab to show it, click its `×` to close it, and click `+` to open a new
one. When the tabs do not fit in the window, turn the mouse wheel over the tab
bar to scroll through them. A tab you have not named shows the title of its
active pane, as set by the program running there, and `Tab n` when that pane
has no title, where `n` is the tab's position in the tab bar. A new tab starts
in the working directory of the active pane, like a split.

Double-click a tab, or press the `rename-tab` key, to rename it in place.
`Enter` or a click anywhere else keeps the new name, and `Escape` keeps the old
one. A tab whose label you leave as it was stays unnamed and keeps following
its pane's title. Leave the field empty to go back to the automatic label.
Drag a tab to move it; the numbers of unnamed tabs without a title follow their
new positions.

## Resizing panes

The resize keys move one divider of the active pane 5 cells in the key's
direction. They are repeatable: hold the key to keep moving the divider (see
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
axis does nothing. The resize keys work in vi mode too.

## Working directory of a new pane

A pane made by a split starts in the working directory of the pane it was split
from:

- On macOS and Linux, orzma asks the system for the directory of the pane's
  foreground program. If the system does not report one, orzma uses the directory the
  shell last reported with the OSC 7 or OSC 9;9 escape sequence, and then the
  directory the pane started in.
- On Windows, the directory the shell last reported comes first. orzma makes
  PowerShell (`pwsh` and `powershell`) and `cmd` report it automatically; set
  `shell_integration = false` in the [`[orzma]`](configuration.md#orzma-shell_integration) table
  to turn this off.

orzma uses the directory only if it still exists; otherwise, and for the first
pane, the new pane starts in your home directory.

## Inactive panes

Panes other than the active pane are drawn with a tint. Change or turn off the
effect in the [`[inactive_pane]`](configuration.md#inactive_pane) table.
