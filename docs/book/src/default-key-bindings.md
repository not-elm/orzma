# Default Key Bindings

These are the keys orzma binds when your configuration leaves them alone. To
change them, see [Key Bindings](key-bindings.md). Windows and Linux share the
same defaults, and only the [general shortcuts](#general) differ on macOS.

## General

| Action | macOS | Windows and Linux | What it does |
| --- | --- | --- | --- |
| `leader` | `Cmd` (tap) | `Alt` (tap) | The key that starts a `<Leader>` binding (see [The leader key](key-bindings.md#the-leader-key)). |
| `paste` | `Cmd+V` | `Ctrl+V` | Paste from the system clipboard. |
| `copy` | `Cmd+C` | `Ctrl+C` | Copy the active pane's selection to the system clipboard, then dismiss the selection. |
| `increase-font-size` | `r:Cmd+Plus` | `r:Ctrl+Plus` | Step the terminal font size up. |
| `decrease-font-size` | `r:Cmd+-` | `r:Ctrl+-` | Step the terminal font size down. |
| `reset-font-size` | `Cmd+0` | `Ctrl+0` | Return the terminal font size to [`[font] size`](configuration.md#font-size). |
| `release-webview-focus` | `<Leader>u` | `<Leader>u` | Return keyboard focus from a focused webview to the terminal. |
| `enter-vi-mode` | `Alt+s` | `Alt+s` | Enter [vi mode](vi-mode.md). |
| `quit` | `Cmd+Q` | unbound | Quit orzma. |

`r:` marks a binding that keeps firing while you hold it (see
[Repeatable bindings](key-bindings.md#repeatable-bindings-r)).

`quit` ships unbound on Windows and Linux because the window manager's own
close shortcut (`Alt+F4` on Windows) already exits orzma. Bind it explicitly if
you want a second way out.

On Windows and Linux, binding `paste` to `Ctrl+V` takes that key away from the
program running in the terminal, so readline's quoted-insert and vim's
visual-block mode no longer see it. Set `paste = "Ctrl+Shift+V"` to give it
back.

A copy chord that uses `Ctrl` alone, such as the stock `Ctrl+C`, copies only
while text is selected; with nothing selected, it reaches the program as usual,
so `Ctrl+C` still interrupts. A copy chord with any other modifier, such as the
macOS `Cmd+C`, always copies.

## Panes

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
| `resize-left-pane` | `r:Alt+Shift+H` | Move a divider of the active pane 5 cells left, repeatable (see [Resizing panes](panes-and-workspaces.md#resizing-panes)). |
| `resize-down-pane` | `r:Alt+Shift+J` | Move a divider of the active pane 5 cells down, repeatable (see [Resizing panes](panes-and-workspaces.md#resizing-panes)). |
| `resize-up-pane` | `r:Alt+Shift+K` | Move a divider of the active pane 5 cells up, repeatable (see [Resizing panes](panes-and-workspaces.md#resizing-panes)). |
| `resize-right-pane` | `r:Alt+Shift+L` | Move a divider of the active pane 5 cells right, repeatable (see [Resizing panes](panes-and-workspaces.md#resizing-panes)). |

<!-- ANCHOR_END: pane-actions -->

## Workspaces

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

## Vi mode

These keys work only in [vi mode](vi-mode.md). They are set in the `[vi-mode]`
table (see [Vi mode keys](key-bindings.md#vi-mode-keys-vi-mode)).

<!-- ANCHOR: vi-mode-actions -->

| Action | Default | What it does |
| --- | --- | --- |
| `cursor-left` | `h`, `ArrowLeft` | Move the cursor one cell left. |
| `cursor-down` | `j`, `ArrowDown` | Move the cursor one cell down. |
| `cursor-up` | `k`, `ArrowUp` | Move the cursor one cell up. |
| `cursor-right` | `l`, `ArrowRight` | Move the cursor one cell right. |
| `line-start` | `0` | Jump to column 0 (of the first row, on a wrapped line). |
| `line-end` | `$` | Jump to the last non-blank column; pressed again, jump to the last column, or to the end of the text on a wrapped line. |
| `line-first-char` | `^` | Jump to the first non-blank column. |
| `next-word` | `w` | Jump to the next (semantic) word start. |
| `previous-word` | `b` | Jump to the previous (semantic) word start. |
| `next-word-end` | `e` | Jump to the next (semantic) word end. |
| `next-space` | `W` | Jump to the next space-delimited word start. |
| `previous-space` | `B` | Jump to the previous space-delimited word start. |
| `next-space-end` | `E` | Jump to the next space-delimited word end. |
| `screen-top` | `H` | Jump to the top visible line. |
| `screen-middle` | `M` | Jump to the middle visible line. |
| `screen-bottom` | `L` | Jump to the bottom visible line. |
| `previous-paragraph` | `{` | Jump to the previous paragraph boundary. |
| `next-paragraph` | `}` | Jump to the next paragraph boundary. |
| `matching-bracket` | `%` | Jump to the bracket matching the one under the cursor. |
| `history-top` | `g` | Scroll to the oldest history line. |
| `history-bottom` | `G` | Scroll to the live tail. |
| `page-up` | `Ctrl+B` | Scroll one page up. |
| `page-down` | `Ctrl+F` | Scroll one page down. |
| `half-page-up` | `Ctrl+U` | Scroll half a page up. |
| `half-page-down` | `Ctrl+D` | Scroll half a page down. |
| `scroll-up` | `Ctrl+Y` | Scroll one line up. |
| `scroll-down` | `Ctrl+E` | Scroll one line down. |
| `toggle-selection` | `v`, `Space` | Toggle a character-wise selection. |
| `toggle-line-selection` | `V` | Toggle a line-wise selection. |
| `toggle-rect-selection` | `Ctrl+V` | Toggle a rectangular selection. Currently selects whole lines. |
| `yank` | `y`, `Enter` | Copy the selection to the clipboard and leave vi mode. |
| `exit` | `q`, `Escape`, `Ctrl+C` | Leave vi mode. |
| `search-forward` | `/` | Open the search-down prompt (not implemented yet). |
| `search-backward` | `?` | Open the search-up prompt (not implemented yet). |
| `search-next` | `n` | Repeat the previous search (not implemented yet). |
| `search-previous` | `N` | Repeat the previous search, reversed (not implemented yet). |
| `jump-forward` | `f` | Open the jump-to-char-forward prompt (not implemented yet). |
| `jump-backward` | `F` | Open the jump-to-char-backward prompt (not implemented yet). |
| `jump-to-forward` | `t` | Open the jump-till-char-forward prompt (not implemented yet). |
| `jump-to-backward` | `T` | Open the jump-till-char-backward prompt (not implemented yet). |

<!-- ANCHOR_END: vi-mode-actions -->
