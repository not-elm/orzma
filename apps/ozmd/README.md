# ozmd

A rich Markdown viewer for ozmux panes — a companion app built with the
[`ratatui-ozma`](../../sdk/ratatui-ozma) SDK.

## Overview

ozmd runs inside an ozmux pane and renders Markdown in an embedded webview,
wrapped in native terminal chrome: a status line, an optional outline panel,
and a search line. You drive it from the keyboard like a pager, while the page
handles rich rendering — diagrams, math, and highlighted code.

## Features

- **Live reload** — saving the file re-renders it automatically, and your
  scroll position is preserved across reloads.
- **Syntax highlighting** — fenced code blocks are highlighted with
  highlight.js.
- **Math** — LaTeX rendered with KaTeX (`$inline$` and `$$block$$`).
- **Mermaid diagrams** — ` ```mermaid ` fences render as diagrams.
- **Outline panel** — jump between the document's headings.
- **In-page search** — highlight matches and step through them.

## Installation

ozmd is installed from source alongside the other companion apps:

```bash
just install-apps
```

This requires the ozmux app itself — see the
[root README](../../README.md#installation) for installing ozmux.

## Usage

```bash
ozmd <markdown-file>
```

ozmd must be launched **inside an ozmux pane**. Run anywhere else, it exits
with:

```
ozmd: not inside an ozmux pane: OZMA_SOCK is unset. Run ozmd inside an ozmux pane.
```

The status line at the top shows the file name, the live-reload state, and the
scroll position:

```
ozmd · README.md    ● live    42%
```

`● live` means the file is being watched. If the file is deleted, the status
switches to `○ missing` and the last rendered content stays on screen.

## Keyboard shortcuts

### Reading

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

### Outline panel

| Key | Action |
| --- | --- |
| `j` / `↓` | Move the selection down |
| `k` / `↑` | Move the selection up |
| `Enter` | Jump to the selected heading |
| `o` / `Tab` / `Esc` | Close the panel |
| `q` | Quit |

### Search

| Key | Action |
| --- | --- |
| (type) | Build the query |
| `Backspace` | Delete the last character |
| `Enter` | Run the search |
| `Esc` | Cancel |

After running a search, use `n` / `N` in reading mode to move between matches,
and `Esc` to clear the highlight.

## License

MIT. See [LICENSE](../../LICENSE).
