# orzbrowser

orzbrowser runs inside an orzma pane and loads a remote URL in an embedded
webview below a two-row toolbar. You drive it from the keyboard like a
Vim-style pager — scrolling, following links by typing hint labels, and
stepping through history — and type addresses or search terms into the
toolbar's address bar.

## Features

- **Vim-style scrolling** — `j` / `k` move by line, `Ctrl+d` / `Ctrl+u` by half
  a page, `Ctrl+f` / `Ctrl+b` by a full page, and `gg` / `G` jump to the top /
  bottom. Holding a key keeps scrolling smoothly until you let go. When a frame
  embedded in the page (a code demo, a video player) has keyboard focus, the
  keys scroll that frame, and scroll the page around it once the frame can
  scroll no further that way.
- **Link hints** — press `f` to overlay labels on every link and form field,
  then type a label to follow it. Landing on a text field switches to Insert
  mode automatically.
- **History** — `H` and `L` step back and forward through the session history.
- **Address bar with search** — `o` (or `:`, or a click on the address) opens
  the address bar. Type an address to open it, or any other words to search
  for them. The caret moves with the arrow keys, and input methods work, so
  you can search in any language.
- **Modal input** — Normal, Insert, Address, Hint, and Help modes. In Normal
  mode the page holds keyboard focus and the scroll keys scroll it; `i` enters
  Insert mode, where every key but `Escape` types into the page, and `Escape` returns
  to Normal.
- **In-app help** — `?` shows the full shortcut list.

## Usage

```bash
orzbrowser [address or search terms]
```

`orzbrowser github.com` opens a site, and `orzbrowser rust async` searches for
the words. With no argument, orzbrowser opens the search engine's front page
with the address bar ready for typing.

The toolbar shows the mode, the address with its host in bold, and the main
keys of the mode. An `http` address shows **Not secure** before its host.

## Address bar

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

Pressing Enter on the current address reloads the page. `Escape` closes the
address bar without navigating. While the address bar is open, `Ctrl+c` does
not quit; press `Escape` and then `q`.

Searches go to DuckDuckGo. To use another engine, set `ORZBROWSER_SEARCH_URL`
to its search URL with `{}` where the search terms go:

```bash
export ORZBROWSER_SEARCH_URL='https://www.google.com/search?q={}'
```

The URL must be `http` or `https` and contain exactly one `{}`, in its path,
query, or fragment; otherwise orzbrowser exits with an error.

## Keyboard shortcuts

### Normal

| Key | Action |
| --- | --- |
| `j` / `ArrowDown` | Scroll down one line |
| `k` / `ArrowUp` | Scroll up one line |
| `Ctrl+d` / `Space` | Scroll half a page down |
| `Ctrl+u` | Scroll half a page up |
| `Ctrl+f` / `PageDown` | Scroll a full page down |
| `Ctrl+b` / `PageUp` | Scroll a full page up |
| `gg` | Jump to the top |
| `G` | Jump to the bottom |
| `H` | History back |
| `L` | History forward |
| `o` / `:` | Open the address bar |
| `r` | Reload the page |
| `i` | Insert mode (type into the page) |
| `f` | Follow a link (show hints) |
| `?` | Show help |
| `q` | Quit |

In Normal mode the page holds keyboard focus, so `Ctrl+c` copies the page's
selection instead of quitting (on macOS, copy with `Cmd+c`); quit with `q`.
`Ctrl+c` quits only while the pane is too short to show the page.

### Address bar

| Key | Action |
| --- | --- |
| (type) | Edit the address; the arrow keys, `Home`, and `End` move the caret |
| `Enter` | Open the address, or search for the words |
| `Escape` | Cancel |

### Hint

| Key | Action |
| --- | --- |
| (type label) | Narrow to and follow the matching hint |
| `Backspace` | Delete the last label character |
| `Escape` | Cancel hints |
| `Ctrl+c` | Quit |

### Insert

| Key | Action |
| --- | --- |
| `Escape` | Return to Normal mode |

In Insert mode every other key goes to the page, so you can type into focused
inputs. orzma's own shortcuts still run first (see
[Key Bindings](key-bindings.md#shortcuts-while-a-webview-has-focus)).

### Help

| Key | Action |
| --- | --- |
| `Escape` / `q` | Close help |
| `Ctrl+c` | Quit |

## Acknowledgements

orzbrowser's keyboard model and link-hint workflow are inspired by
[Vimium](https://github.com/philc/vimium), the keyboard-driven browser
extension. The hint alphabet (`sadfjklewcmpgh`) is Vimium's default. Vimium is
distributed under the
[MIT License](https://github.com/philc/vimium/blob/master/MIT-LICENSE.txt);
the scrolling of orzbrowser and orzmd is ported from Vimium's
`content_scripts/scroller.js` (commit `e34b529328`), and its license notice is
reproduced in
`THIRD-PARTY-LICENSES.md`; the rest of orzbrowser is an independent
implementation.
