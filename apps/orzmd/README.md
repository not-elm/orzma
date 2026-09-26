# orzmd

A rich Markdown viewer for orzma panes — a companion app built with the
[`ratatui_orzma`](../../sdk/ratatui_orzma) SDK.

orzmd renders Markdown in an embedded webview — diagrams, math, and highlighted
code — inside terminal chrome that you drive from the keyboard like a pager. It
re-renders the file when you save it.

## Installation

orzmd comes with orzma. To build it from source, run `just install-apps` from
the repository root.

## Usage

```bash
orzmd <markdown-file>
```

Run it inside an orzma pane. See the
[user guide](https://not-elm.github.io/orzma/companion-apps.html#orzmd) for its
features and keyboard shortcuts.

## License

MIT. See [LICENSE](../../LICENSE).
