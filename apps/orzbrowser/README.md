# orzbrowser

A keyboard-driven TUI browser for orzma panes — a companion app built with the
[`ratatui-orzma`](../../sdk/ratatui-orzma) SDK.

## Overview

orzbrowser runs inside an orzma pane and loads a remote URL in an embedded
webview, wrapped in native terminal chrome: a status line, an address bar, and
a help modal. You drive it from the keyboard like a Vim-style pager — scrolling,
following links by typing hint labels, and stepping through history — while the
page renders in the webview.

## Features

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

## Installation

orzbrowser is installed from source alongside the other companion apps:

```bash
just install-apps
```

This requires the orzma app itself — see the
[root README](../../README.md#installation) for installing orzma.

## Usage

```bash
orzbrowser <url>
```

orzbrowser must be launched **inside an orzma pane**. Run anywhere else, it exits
with:

```
orzbrowser: not inside an orzma pane: ORZMA_SOCK is unset. Run orzbrowser inside an orzma pane.
```

The status line at the top shows the current mode and the loaded URL:

```
[Normal] https://example.com
```

Opening the address bar with `o` or `:` replaces it with an editable prompt:

```
> https://example.com_
```

## Keyboard shortcuts

### Normal

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

### Address bar

| Key | Action |
| --- | --- |
| (type) | Edit the URL |
| `Backspace` | Delete the last character |
| `Enter` | Navigate to the URL |
| `Esc` | Cancel |
| `Ctrl-c` | Quit |

### Hint

| Key | Action |
| --- | --- |
| (type label) | Narrow to and follow the matching hint |
| `Backspace` | Delete the last label character |
| `Esc` | Cancel hints |
| `Ctrl-c` | Quit |

### Insert

| Key | Action |
| --- | --- |
| `Esc` | Return to Normal mode |

In Insert mode every other key goes to the page, so you can type into focused
inputs.

### Help

| Key | Action |
| --- | --- |
| `Esc` / `q` | Close help |
| `Ctrl-c` | Quit |

## Acknowledgements

orzbrowser's keyboard model and link-hint workflow are inspired by
[Vimium](https://github.com/philc/vimium), the keyboard-driven browser
extension. The hint alphabet (`sadfjklewcmpgh`) is Vimium's default. Vimium is
distributed under the
[MIT License](https://github.com/philc/vimium/blob/master/MIT-LICENSE.txt);
orzbrowser ships an independent implementation rather than Vimium source.

## License

MIT. See [LICENSE](../../LICENSE).
