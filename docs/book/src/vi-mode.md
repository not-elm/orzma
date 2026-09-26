# Vi Mode

Vi mode moves a cursor over the pane's screen and scrollback with vi keys, so
you can select and copy text without the mouse. Press `<Leader>s` to enter it.
Press `y` or `Enter` to copy the selection and leave, or `q` or `Escape` to
leave without copying.

> [!NOTE]
> The search and jump actions (`/`, `?`, `n`, `N`, `f`, `F`, `t`, `T`) are not
> implemented yet: vi mode swallows those keys and does nothing.
> `toggle-rect-selection` (`Ctrl+V`) currently selects whole lines, because
> rectangular selection is not implemented yet.

## Keys

A `[vi-mode]` entry is an optional `Ctrl+` prefix plus exactly one key.

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
  of layout or case.
- **Values** are a single key string (`yank = "Y"`) or an array of key
  strings (`exit = ["q", "Escape", "Ctrl+C"]`); any action can be bound to
  zero, one, or several keys.
- **`""` or `[]` unbinds** an action (for example, `search-forward = ""`).
- **Duplicate keys are a startup error**: if the same key string is bound to
  more than one `[vi-mode]` action, orzma fails at startup naming every
  colliding action (as duplicate chords are in `[shortcuts]`). An unknown
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

- `paste` does nothing in vi mode, so a direct paste chord passes through —
  the stock `Ctrl+V` reaches `toggle-rect-selection`.
- `copy` passes a Ctrl-only chord through whenever there is no selection to
  copy — the stock `Ctrl+C` reaches `exit`.

## Actions

| Action | Default | What it does |
| --- | --- | --- |
| `cursor-left` | `h`, `ArrowLeft` | Move the cursor one cell left. |
| `cursor-down` | `j`, `ArrowDown` | Move the cursor one cell down. |
| `cursor-up` | `k`, `ArrowUp` | Move the cursor one cell up. |
| `cursor-right` | `l`, `ArrowRight` | Move the cursor one cell right. |
| `line-start` | `0` | Jump to column 0. |
| `line-end` | `$` | Jump to the last column. |
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
| `matching-bracket` | `%` | Jump to the matching bracket. |
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

## Escape and unbound keys

By default, `Escape` is bound to the `exit` action, which leaves vi mode
entirely. To deselect a selection in orzma without leaving vi mode, press `v`
(toggle-selection is a toggle: with a selection active, it clears it).

Keys not bound to any `[vi-mode]` action are swallowed while vi mode is
active (they never reach the pane) — this includes stock `copy-mode-vi` keys
that orzma does not carry over by default, such as `:` (goto-line), digit
repeat prefixes, `o` (other-end), `A` (append-and-cancel), `X` / `M-x`
(mark), `;` / `,` (jump repeat), `z` (scroll-middle), and `D`
(copy-end-of-line-and-cancel). Bind them to a `[vi-mode]` action yourself
if you need them; more built-in actions may be added later.

## Example

The stock `[vi-mode]` table:

```toml
[vi-mode]
# Vi-mode key bindings. See "Keys" above for the key
# syntax and the duplicate-key rule.

# --- cursor motion (trailing comment: ViMotion variant / copy-mode command, for reference) ---
cursor-left        = ["h", "ArrowLeft"]     # Left            / cursor-left
cursor-down        = ["j", "ArrowDown"]     # Down            / cursor-down
cursor-up          = ["k", "ArrowUp"]       # Up              / cursor-up
cursor-right       = ["l", "ArrowRight"]    # Right           / cursor-right
line-start         = ["0"]                  # First           / start-of-line
line-end           = ["$"]                  # Last            / end-of-line
line-first-char    = ["^"]                  # FirstOccupied   / back-to-indentation
next-word          = ["w"]                  # SemanticRight   / next-word
previous-word      = ["b"]                  # SemanticLeft    / previous-word
next-word-end      = ["e"]                  # SemanticRightEnd / next-word-end
next-space         = ["W"]                  # WordRight       / next-space
previous-space     = ["B"]                  # WordLeft        / previous-space
next-space-end     = ["E"]                  # WordRightEnd    / next-space-end
screen-top         = ["H"]                  # High            / top-line
screen-middle      = ["M"]                  # Middle          / middle-line
screen-bottom      = ["L"]                  # Low             / bottom-line
previous-paragraph = ["{"]                  # ParagraphUp     / previous-paragraph
next-paragraph     = ["}"]                  # ParagraphDown   / next-paragraph
matching-bracket   = ["%"]                  # Bracket         / next-matching-bracket

# --- scrolling ---
history-top        = ["g"]                  # Top      / history-top
history-bottom     = ["G"]                  # Bottom   / history-bottom
page-up            = ["Ctrl+B"]             # PageUp   / page-up
page-down          = ["Ctrl+F"]             # PageDown / page-down
half-page-up       = ["Ctrl+U"]             # HalfUp   / halfpage-up
half-page-down     = ["Ctrl+D"]             # HalfDown / halfpage-down
scroll-up          = ["Ctrl+Y"]             # LineUp   / scroll-up
scroll-down        = ["Ctrl+E"]             # LineDown / scroll-down

# --- selection ---
toggle-selection      = ["v", "Space"]      # Simple / begin-selection
toggle-line-selection = ["V"]               # Lines  / select-line
toggle-rect-selection = ["Ctrl+V"]          # Block  / rectangle-toggle

# --- copy / exit ---
yank = ["y", "Enter"]                       # copy the selection, then leave vi mode
exit = ["q", "Escape", "Ctrl+C"]            # leave vi mode

# --- search / jump (not implemented yet — see the note at the top) ---
search-forward     = ["/"]                  # opens a prompt -> -X search-forward
search-backward    = ["?"]                  # opens a prompt -> -X search-backward
search-next        = ["n"]                  # repeats the last search -> -X search-again
search-previous    = ["N"]                  # repeats it reversed -> -X search-reverse
jump-forward       = ["f"]                  # opens a prompt -> -X jump-forward
jump-backward      = ["F"]                  # opens a prompt -> -X jump-backward
jump-to-forward    = ["t"]                  # opens a prompt -> -X jump-to-forward
jump-to-backward   = ["T"]                  # opens a prompt -> -X jump-to-backward
```
