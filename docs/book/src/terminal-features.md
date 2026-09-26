# Terminal Features

## Selection and clipboard

Drag with the left mouse button to select text. When you release the button,
orzma copies the selection to the clipboard and keeps it highlighted. A
triple-click, or a click with `Alt` (`Option` on macOS) held, selects whole
lines.

> [!NOTE]
> Selecting a word with a double-click and rectangular selection are not
> implemented yet: a double-click starts an ordinary selection, and an
> `Alt`+click selects whole lines.

| Action | macOS | Windows and Linux |
| --- | --- | --- |
| Copy the selection | `Cmd+C` | `Ctrl+C` |
| Paste | `Cmd+V` | `Ctrl+V` |

Copying with the shortcut also clears the selection. To change these keys, see
[Key Bindings](key-bindings.md).

`Ctrl+C` copies only while a selection exists. With nothing selected it is sent
to the shell as the interrupt byte (`0x03`) instead, and the `copy` shortcut
dismisses the selection, so pressing `Ctrl+C` twice copies and then interrupts.
The mouse's own copy-on-release keeps the selection highlighted, so a drag
followed by `Ctrl+C` still copies. A copy chord that carries any other
modifier — including the macOS `Cmd+C` default — always copies and never
reaches the shell. Typing or pasting into the shell — including a confirmed
IME composition — also dismisses the selection, while modifier keys on their
own, other shortcuts, and the mouse wheel leave it in place.

While an application tracks the mouse (for example nvim with `mouse` set),
clicks and drags go to the application instead of orzma's own selection;
holding Shift as the button goes down makes that click or drag select in
orzma instead, and a drag copies on release. Shift only matters at the
press: pressing or letting go of it mid-drag does not reroute the drag. A
pane scrolled back into its history always selects in orzma too, whether
or not Shift is held.

## Scrolling

Turn the mouse wheel to scroll back through the pane's history: three lines
per notch, or one line per notch while `Alt` is held. A program that tracks
the mouse, such as an editor with mouse support, receives the wheel itself;
hold `Shift` to bypass the program's mouse handling. A full-screen program
that does not track the mouse, such as `less`, receives the wheel as up and
down arrow keys. The amounts and the modifier are set in the
[`[mouse]`](configuration.md#mouse) table.

## Font zoom

| Action | macOS | Windows and Linux |
| --- | --- | --- |
| Make text larger | `Cmd+Plus` | `Ctrl+Plus` |
| Make text smaller | `Cmd+-` | `Ctrl+-` |
| Reset the size | `Cmd+0` | `Ctrl+0` |

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
rebind the zoom actions to `<Leader>`-scoped chords in
[Key Bindings](key-bindings.md), which fire regardless of focus.

The zoom factor is not remembered across restarts; set `[font] size` to change
the size permanently.

## Hyperlinks

Programs can turn text into a link with the OSC 8 escape sequence. Hold `Cmd`
on macOS or `Ctrl` on Windows and Linux and click a link to open it with the system's
default handler; the pointer turns into a hand while the key is held over a
link. orzma opens only `http`, `https`, `mailto`, and `ftp` links.
