# orzbrowser

A keyboard-driven TUI browser for orzma panes — a companion app built with the
[`ratatui_orzma`](../../sdk/ratatui_orzma) SDK.

orzbrowser loads a remote URL in an embedded webview inside terminal chrome that
you drive from the keyboard: Vim-style scrolling, link hints, history, and an
address bar.

## Installation

orzbrowser comes with orzma. To build it from source, run `just install-apps`
from the repository root.

## Usage

```bash
orzbrowser <url>
```

Run it inside an orzma pane. See the
[user guide](https://not-elm.github.io/orzma/companion-apps.html#orzbrowser)
for its features and keyboard shortcuts.

## Acknowledgements

orzbrowser's keyboard model and link-hint workflow are inspired by
[Vimium](https://github.com/philc/vimium). orzbrowser ships an independent
implementation rather than Vimium source.

## License

MIT. See [LICENSE](../../LICENSE).
