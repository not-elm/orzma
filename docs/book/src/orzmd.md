# orzmd

orzmd runs inside an orzma pane and renders Markdown in an embedded webview
with its own chrome: a rail at the top, an optional outline sidebar, and a
find box. You drive it from the keyboard like a pager, while the page handles
rich rendering — diagrams, math, and highlighted code.

## Features

- **Live reload** — saving the file re-renders it automatically, and your
  scroll position is preserved across reloads.
- **Syntax highlighting** — fenced code blocks are highlighted with
  highlight.js.
- **Math** — LaTeX rendered with KaTeX (`$inline$` and `$$block$$`).
- **Mermaid diagrams** — ` ```mermaid ` fences render as diagrams.
- **GitHub alerts** — `> [!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]`, and
  `[!CAUTION]` blockquotes render as colored callout boxes with icons.
- **Outline sidebar** — jump between the document's headings; it opens on the
  section you are reading.
- **In-page search** — matches highlight as you type, and you step through them.

## Usage

```bash
orzmd <markdown-file>
```

The rail at the top shows the file name and the heading you are reading,
after its parent headings:

```
configuration.md › Settings › [inactive_pane]
```

If the file is deleted, the rail shows a red **File deleted** badge and the
last rendered content stays on screen. Messages such as
`cannot open ../notes.md` appear at the bottom of the page and disappear after
four seconds or at your next key press.

On Windows, local images referenced by a document are staged as symlinks when
Windows allows it (Developer Mode is on, or orzmd runs as administrator) and
copied otherwise; a copied image does not refresh until orzmd is restarted.

## Keyboard shortcuts

### Reading

| Key | Action |
| --- | --- |
| `j` / `ArrowDown` | Scroll down one line |
| `k` / `ArrowUp` | Scroll up one line |
| `Ctrl+d` / `Ctrl+u` | Scroll half a page down / up |
| `Ctrl+f` / `Ctrl+b` | Scroll a full page down / up |
| `Space` / `PageDown` | Scroll a full page down |
| `PageUp` | Scroll a full page up |
| `gg` | Jump to the top |
| `G` | Jump to the bottom |
| `]]` / `[[` | Jump to the next / previous heading |
| `o` / `Tab` | Toggle the outline sidebar |
| `/` | Start a search |
| `n` / `N` | Next / previous match (after a search) |
| `r` | Reload the file |
| `Backspace` / `Ctrl+o` | Go back to the previous document |
| `q` | Quit |

The page handles every key and keeps keyboard focus while you read, so
`Ctrl+c` does not quit while the page has focus: on Windows and Linux it copies
the page's selection, and on macOS you copy with `Cmd+c`. Quit with `q`. While
the page does not have focus — before it has loaded, or after you come back to
the pane with the keyboard — `Ctrl+c` still quits. Holding a scroll key keeps
the page scrolling smoothly until you release it, and each tap moves exactly
one step. While a search query is being typed, every key, `q` included, goes
into the find box.

### Outline sidebar

| Key | Action |
| --- | --- |
| `j` / `ArrowDown` | Move the selection down |
| `k` / `ArrowUp` | Move the selection up |
| `Enter` | Jump to the selected heading |
| Click a heading | Jump to that heading |
| `o` / `Tab` / `Escape` | Close the sidebar |
| `/` | Start a search, keeping the sidebar open |
| `r` / `Backspace` / `Ctrl+o` / `q` | Reload / go back / quit, as in reading |

The outline opens with the section you are reading selected.

### Search

| Key | Action |
| --- | --- |
| (type) | Build the query; matches highlight as you type |
| `Enter` | Keep the matches and return to reading; with no match, nothing happens |
| `Escape` | Cancel the search and return to where it started |

After `Enter`, use `n` / `N` in reading mode to move between matches, and
`Escape` to clear the highlight. The search ignores case unless the query
contains an uppercase letter.

If the find box loses focus while you type — for example, you click another
pane — orzmd keeps the matches when there are any and otherwise closes the find
box; either way the page stays where it is.
