# orzbrowser

A keyboard-driven TUI browser for orzma panes — a companion app built with the
[`ratatui_orzma`](../../sdk/ratatui_orzma) SDK.

orzbrowser loads a remote URL in an embedded webview below a toolbar that
you drive from the keyboard: Vim-style scrolling, link hints, history, and an
address bar.

## Installation

orzbrowser comes with orzma. To build it from source, run `just install-apps`
from the repository root.

## Usage

```bash
orzbrowser [address or search terms]
```

Run it inside an orzma pane. `orzbrowser github.com` opens a site,
`orzbrowser rust async` searches for the words, and `orzbrowser` alone opens
the search engine with the address bar ready. See the
[user guide](https://not-elm.github.io/orzma/companion-apps.html#orzbrowser)
for its features, the address bar, and keyboard shortcuts.

## Acknowledgements

orzbrowser's keyboard model and link-hint workflow are inspired by
[Vimium](https://github.com/philc/vimium). orzbrowser ships an independent
implementation rather than Vimium source.

## License

MIT. See [LICENSE](../../LICENSE).
