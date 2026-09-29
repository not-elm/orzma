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

## Keys

Vi-mode keys are set in the [`[vi-mode]`](configuration.md#vi-mode) table of
the configuration file. A `[vi-mode]` entry is an optional `Ctrl+` prefix plus
exactly one key.

- **Keys** are either a single character, matched **case-sensitively**
  (`"w"` and `"W"` are different bindings — Shift is expressed through the
  character's case, e.g. `"W"` means Shift+w, not `"Shift+w"`), or one of the
  named keys `Escape` `Enter` `Space` `Tab` `Backspace` `ArrowUp` `ArrowDown`
  `ArrowLeft` `ArrowRight`.
- **`Ctrl+` is the only modifier prefix accepted.** `Cmd+`, `Alt+`, `Shift+`
  (and their aliases) are parse errors inside `[vi-mode]` — Shift is
  expressed via character case as above, and Cmd/Alt chords are reserved for
  application shortcuts (`[shortcuts]`); vi mode does not match keystrokes with Cmd or Alt held.
- After `Ctrl+`, the key must be an ASCII alphanumeric character or a named
  key — `Ctrl+$` is a parse error. `Ctrl+` entries match on the physical key
  pressed (not the character it produces), so they behave the same regardless
  of layout or case, and they do not match while Shift is also held.
- **Values** are a single key string (`yank = "Y"`) or an array of key
  strings (`exit = ["q", "Escape", "Ctrl+C"]`); any action can be bound to
  zero, one, or several keys.
- **`""` or `[]` unbinds** an action (for example, `search-forward = ""`).
- **Duplicate keys are a startup error**: if the same key, however it is
  spelled (`"Space"` and `" "`, `"Ctrl+F"` and `"ctrl+f"`), is bound more than
  once in `[vi-mode]`, orzma fails at startup naming every colliding action
  (as duplicate chords are in `[shortcuts]`). An unknown
  action name, like the parse errors above, makes orzma ignore the whole
  file instead (see [Validation](configuration.md#validation)).

Shadowing note: `[shortcuts]` chords (both leader-scoped and direct) are
matched **before** `[vi-mode]` keys. If the same keystroke is bound in both
tables, the `[shortcuts]` action normally fires and the `[vi-mode]` binding
never sees it — e.g. setting `leader = "Ctrl+B"` shadows the default
`page-up = "Ctrl+B"` binding while vi mode is active. orzma does not
validate across the two tables; check your own bindings for overlap.

Two actions decline the keystroke instead of shadowing it, so the `[vi-mode]`
binding still runs:

- A direct `paste` chord does nothing in vi mode, so it passes through — the
  stock `Ctrl+V` on Windows and Linux reaches `toggle-rect-selection`. A
  `<Leader>`-scoped `paste` binding still pastes.
- `copy` passes a Ctrl-only chord through whenever there is no selection to
  copy — the stock `Ctrl+C` on Windows and Linux reaches `exit`. With a
  selection, it copies and clears the selection without leaving vi mode.

## Actions

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

The semantic word motions (`w`, `b`, `e`) stop at whitespace and at the
characters in [`[selection] semantic_escape_chars`](configuration.md#selection).

## Escape and unbound keys

By default, `Escape` is bound to the `exit` action, which leaves vi mode
entirely. To deselect without leaving vi mode, press the toggle key that
matches the selection's kind: `v` (or `Space`) clears a character-wise
selection and `V` a line-wise one. The other key switches the selection to its
own kind instead of clearing it.

Keys not bound to any `[vi-mode]` action are swallowed while vi mode is
active (they never reach the pane) — this includes stock `copy-mode-vi` keys
that orzma does not carry over by default, such as `:` (goto-line), digit
repeat prefixes, `o` (other-end), `A` (append-and-cancel), `X` / `M-x`
(mark), `;` / `,` (jump repeat), `z` (scroll-middle), and `D`
(copy-end-of-line-and-cancel). orzma has no actions for these; you can only
bind such a key to one of the actions above.
