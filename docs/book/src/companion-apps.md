# Companion Apps

orzma comes with two terminal apps that show web pages inside the terminal,
built with the `ratatui_orzma` SDK: **orzmd**, a Markdown viewer, and
**orzbrowser**, a keyboard-driven browser. Homebrew links both into your `PATH`, and the
Windows installer and the Linux `.deb` add them to it. The macOS dmg keeps them
inside `orzma.app`, and the Linux tarball keeps them in `~/.local/share/orzma`;
see [Getting Started](getting-started.md#install) to add them to your `PATH`.
To build them from a clone of the repository instead, run `just install-apps`.

Both run only inside an orzma pane. Anywhere else, they exit with an error
such as:

```text
orzmd: not inside an orzma pane: ORZMA_SOCK is unset. Run orzmd inside an orzma pane.
```

## orzmd

orzmd runs inside an orzma pane and renders Markdown in an embedded webview
with its own chrome: a rail at the top, an optional outline sidebar, and a
find box. You drive it from the keyboard like a pager, while the page handles
rich rendering — diagrams, math, and highlighted code.

### Features

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

### Usage

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

### Keyboard shortcuts

#### Reading

| Key | Action |
| --- | --- |
| `j` / `↓` | Scroll down one line |
| `k` / `↑` | Scroll up one line |
| `Ctrl-d` / `Ctrl-u` | Scroll half a page down / up |
| `Ctrl-f` / `Ctrl-b` | Scroll a full page down / up |
| `Space` / `PageDown` | Scroll a full page down |
| `PageUp` | Scroll a full page up |
| `gg` | Jump to the top |
| `G` | Jump to the bottom |
| `]]` / `[[` | Jump to the next / previous heading |
| `o` / `Tab` | Toggle the outline sidebar |
| `/` | Start a search |
| `n` / `N` | Next / previous match (after a search) |
| `r` | Reload the file |
| `q` / `Ctrl-c` | Quit |

While the page has keyboard focus, `Ctrl-c` copies in the page instead of
quitting; `q` still quits, because orzmd forwards it to the TUI even while
the page is focused. The exception is typing a search query: then every key,
`q` included, goes into the find box.

#### Outline sidebar

| Key | Action |
| --- | --- |
| `j` / `↓` | Move the selection down |
| `k` / `↑` | Move the selection up |
| `Enter` | Jump to the selected heading |
| Click a heading | Jump to that heading |
| `o` / `Tab` / `Esc` | Close the sidebar |
| `q` | Quit |

The outline opens with the section you are reading selected.

#### Search

| Key | Action |
| --- | --- |
| (type) | Build the query; matches highlight as you type |
| `Enter` | Keep the matches and return to reading; with no match, nothing happens |
| `Esc` | Cancel the search and return to where it started |

After `Enter`, use `n` / `N` in reading mode to move between matches, and
`Esc` to clear the highlight. The search ignores case unless the query
contains an uppercase letter.

## orzbrowser

orzbrowser runs inside an orzma pane and loads a remote URL in an embedded
webview below a two-row toolbar. You drive it from the keyboard like a
Vim-style pager — scrolling, following links by typing hint labels, and
stepping through history — and type addresses or search terms into the
toolbar's address bar.

### Features

- **Vim-style scrolling** — `j` / `k` move by line, `Ctrl-d` / `Ctrl-u` by half
  a page, `Ctrl-f` / `Ctrl-b` by a full page, and `gg` / `G` jump to the top /
  bottom.
- **Link hints** — press `f` to overlay labels on every link and form field,
  then type a label to follow it. Landing on a text field switches to Insert
  mode automatically.
- **History** — `H` and `L` step back and forward through the session history.
- **Address bar with search** — `o` (or `:`, or a click on the address) opens
  the address bar. Type an address to open it, or any other words to search
  for them. The caret moves with the arrow keys, and input methods work, so
  you can search in any language.
- **Modal input** — Normal, Insert, Address, Hint, and Help modes. `i` hands
  keyboard focus to the page so you can type into it; `Esc` returns to Normal.
- **In-app help** — `?` shows the full shortcut list.

### Usage

```bash
orzbrowser [address or search terms]
```

`orzbrowser github.com` opens a site, and `orzbrowser rust async` searches for
the words. With no argument, orzbrowser opens the search engine's front page
with the address bar ready for typing.

The toolbar shows the mode, the address with its host in bold, and the main
keys of the mode. An `http` address shows **Not secure** before its host.

### Address bar

The address bar decides what Enter does as you type, and shows it on the right
(`↵ Open github.com` or `↵ Search DuckDuckGo`):

| You type | Enter does |
| --- | --- |
| An `http://` or `https://` URL | Opens it |
| A URL with another scheme, such as `file:///etc` | Nothing; the reason turns red |
| Text with a space, such as `rust async` | Searches for it |
| `localhost`, an IP address, or `name:port` | Opens it over `http` |
| A name with a dot and a letter-only ending, such as `docs.rs/serde` | Opens it over `https` |
| Anything else, such as `rust` or `3.14` | Searches for it |
| `?` followed by words, such as `? node.js` | Searches for the words, even when they look like an address |

Pressing Enter on the current address reloads the page. `Esc` closes the
address bar without navigating. While the address bar is open, `Ctrl-c` does
not quit; press `Esc` and then `q`.

Searches go to DuckDuckGo. To use another engine, set `ORZBROWSER_SEARCH_URL`
to its search URL with `{}` where the search terms go:

```bash
export ORZBROWSER_SEARCH_URL='https://www.google.com/search?q={}'
```

The URL must be `http` or `https` and contain exactly one `{}`, in its path,
query, or fragment; otherwise orzbrowser exits with an error.

### Keyboard shortcuts

#### Normal

| Key | Action |
| --- | --- |
| `j` / `↓` | Scroll down one line |
| `k` / `↑` | Scroll up one line |
| `Ctrl-d` / `Space` | Scroll half a page down |
| `Ctrl-u` | Scroll half a page up |
| `Ctrl-f` / `PageDown` | Scroll a full page down |
| `Ctrl-b` / `PageUp` | Scroll a full page up |
| `gg` | Jump to the top |
| `G` | Jump to the bottom |
| `H` | History back |
| `L` | History forward |
| `o` / `:` | Open the address bar |
| `r` | Reload the page |
| `i` | Insert mode (focus the webview) |
| `f` | Follow a link (show hints) |
| `?` | Show help |
| `q` / `Ctrl-c` | Quit |

While the page or the toolbar has keyboard focus, `Ctrl-c` copies instead of
quitting; `q` still quits, since Normal mode forwards it to orzbrowser even
while either has focus.

#### Address bar

| Key | Action |
| --- | --- |
| (type) | Edit the address; the arrow keys, `Home`, and `End` move the caret |
| `Enter` | Open the address, or search for the words |
| `Esc` | Cancel |

#### Hint

| Key | Action |
| --- | --- |
| (type label) | Narrow to and follow the matching hint |
| `Backspace` | Delete the last label character |
| `Esc` | Cancel hints |
| `Ctrl-c` | Quit |

#### Insert

| Key | Action |
| --- | --- |
| `Esc` | Return to Normal mode |

In Insert mode every other key goes to the page, so you can type into focused
inputs. orzma's own shortcuts still run first (see
[Key Bindings](key-bindings.md#shortcuts-while-a-webview-has-focus)).

#### Help

| Key | Action |
| --- | --- |
| `Esc` / `q` | Close help |
| `Ctrl-c` | Quit |

### Acknowledgements

orzbrowser's keyboard model and link-hint workflow are inspired by
[Vimium](https://github.com/philc/vimium), the keyboard-driven browser
extension. The hint alphabet (`sadfjklewcmpgh`) is Vimium's default. Vimium is
distributed under the
[MIT License](https://github.com/philc/vimium/blob/master/MIT-LICENSE.txt);
orzbrowser ships an independent implementation rather than Vimium source.
