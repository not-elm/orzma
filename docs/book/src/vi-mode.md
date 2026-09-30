# Vi Mode

Vi mode moves a cursor over the pane's screen and scrollback with vi keys, so
you can select and copy text without the mouse. Press `Alt+s` to enter it.
Press `y` or `Enter` to copy the selection and leave, or `q` or `Escape` to
leave without copying.

> [!NOTE]
> The search and jump actions (`/`, `?`, `n`, `N`, `f`, `F`, `t`, `T`) are not
> implemented yet: vi mode swallows those keys and does nothing.
> `toggle-rect-selection` (`Ctrl+V`) currently selects whole lines, because
> rectangular selection is not implemented yet.

The mouse keeps working in vi mode, even while a program such as nvim tracks
the mouse: a click moves the vi cursor, and a drag selects text (and moves the
vi cursor to where the drag ends). A selection started with the mouse can be
extended with the motion keys, while a click or a drag always starts a new
selection. On the primary screen, where the shell runs, the wheel scrolls the
scrollback. On the alternate screen, where full-screen programs such as nvim
and less run, the wheel sends arrow keys to the program.

## Actions

Change these keys in the `[vi-mode]` table of the configuration file; see
[Vi mode keys](key-bindings.md#vi-mode-keys-vi-mode).

{{#include default-key-bindings.md:vi-mode-actions}}

The semantic word motions (`w`, `b`, `e`) stop at whitespace and at the
characters in [`[selection] semantic_escape_chars`](configuration.md#selection).

## Escape and unbound keys

By default, `Escape` is bound to the `exit` action, which leaves vi mode
entirely. To deselect without leaving vi mode, press the toggle key that
matches the selection's kind: `v` (or `Space`) clears a character-wise
selection and `V` a line-wise one. The other key switches the selection to its
own kind instead of clearing it.

Keys not bound to any `[vi-mode]` action are swallowed while vi mode is
active (they never reach the pane) — this includes keys from tmux's vi copy mode (`copy-mode-vi`)
that orzma does not carry over by default, such as `:` (goto-line), digit
repeat prefixes, `o` (other-end), `A` (append-and-cancel), `X` / `M-x`
(mark), `;` / `,` (jump repeat), `z` (scroll-middle), and `D`
(copy-end-of-line-and-cancel). orzma has no actions for these; you can only
bind such a key to one of the actions above.
