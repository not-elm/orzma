# Companion Apps

orzma comes with two terminal apps that show web pages inside the terminal,
built with the `ratatui_orzma` SDK: **orzmd**, a Markdown viewer, and
**orzbrowser**, a keyboard-driven browser. Homebrew links both into your `PATH`, and the
Windows installer and the Linux `.deb` add them to it; the Linux tarball keeps
them in `~/.local/share/orzma` (see [Getting Started](getting-started.md#linux)).
To build them from a clone of the repository instead, run `just install-apps`.

Both run only inside an orzma pane. Anywhere else, they exit with an error
such as:

```text
orzmd: not inside an orzma pane: ORZMA_SOCK is unset. Run orzmd inside an orzma pane.
```

## orzmd

orzmd runs inside an orzma pane and renders Markdown in an embedded webview,
wrapped in native terminal chrome: a status line, an optional outline panel,
and a search line. You drive it from the keyboard like a pager, while the page
handles rich rendering — diagrams, math, and highlighted code.

### Features

- **Live reload** — saving the file re-renders it automatically, and your
  scroll position is preserved across reloads.
- **Syntax highlighting** — fenced code blocks are highlighted with
  highlight.js.
- **Math** — LaTeX rendered with KaTeX (`$inline$` and `$$block$$`).
- **Mermaid diagrams** — ` ```mermaid ` fences render as diagrams.
- **GitHub alerts** — `> [!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]`, and
  `[!CAUTION]` blockquotes render as colored callout boxes with icons.
- **Outline panel** — jump between the document's headings.
- **In-page search** — highlight matches and step through them.

### Usage

```bash
orzmd <markdown-file>
```

The status line at the top shows the file name, the live-reload state, and the
scroll position:

```
orzmd · README.md    ● live    42%
```

`● live` means the file is being watched. If the file is deleted, the status
switches to `○ missing` and the last rendered content stays on screen.

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
| `o` / `Tab` | Toggle the outline panel |
| `/` | Start a search |
| `n` / `N` | Next / previous match (after a search) |
| `r` | Reload the file |
| `q` / `Ctrl-c` | Quit |

While the page has keyboard focus, `Ctrl-c` copies in the page instead of
quitting; `q` still quits, because orzmd forwards it to the TUI even while
the page is focused.

#### Outline panel

| Key | Action |
| --- | --- |
| `j` / `↓` | Move the selection down |
| `k` / `↑` | Move the selection up |
| `Enter` | Jump to the selected heading |
| `o` / `Tab` / `Esc` | Close the panel |
| `q` | Quit |

#### Search

| Key | Action |
| --- | --- |
| (type) | Build the query |
| `Backspace` | Delete the last character |
| `Enter` | Run the search |
| `Esc` | Cancel |

After running a search, use `n` / `N` in reading mode to move between matches,
and `Esc` to clear the highlight.

## orzbrowser

orzbrowser runs inside an orzma pane and loads a remote URL in an embedded
webview, wrapped in native terminal chrome: a status line, an address bar, and
a help modal. You drive it from the keyboard like a Vim-style pager — scrolling,
following links by typing hint labels, and stepping through history — while the
page renders in the webview.

### Features

- **Vim-style scrolling** — `j` / `k` move by line, `Ctrl-d` / `Ctrl-u` by half
  a page, `Ctrl-f` / `Ctrl-b` by a full page, and `gg` / `G` jump to the top /
  bottom.
- **Link hints** — press `f` to overlay labels on every link and form field,
  then type a label to follow it. Landing on a text field switches to Insert
  mode automatically.
- **History** — `H` and `L` step back and forward through the session history.
- **Address bar** — `o` (or `:`) opens the address bar pre-filled with the
  current URL; a scheme-less entry like `github.com` is completed to `https://`.
- **Modal input** — Normal, Insert, Address, Hint, and Help modes. `i` hands
  keyboard focus to the page so you can type into it; `Esc` returns to Normal.
- **In-app help** — `?` shows the full shortcut list.

### Usage

```bash
orzbrowser <url>
```

The status line at the top shows the current mode and the loaded URL:

```
[Normal] https://example.com
```

Opening the address bar with `o` or `:` replaces it with an editable prompt:

```
> https://example.com_
```

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

While the page has keyboard focus, `Ctrl-c` copies in the page instead of
quitting; `q` still quits, since Normal mode forwards it to the TUI even
while the page is focused.

#### Address bar

| Key | Action |
| --- | --- |
| (type) | Edit the URL |
| `Backspace` | Delete the last character |
| `Enter` | Navigate to the URL |
| `Esc` | Cancel |
| `Ctrl-c` | Quit |

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
