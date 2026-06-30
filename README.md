# ozmux (Ozma Terminal Multiplexer)

> [!CAUTION]
> This app is still in early development and may introduce breaking changes.

ozmux is a terminal emulator that can render webviews directly inside the
terminal, with built-in tmux integration.

## Installation

macOS (Apple Silicon) via Homebrew Cask:

```bash
brew install --cask not-elm/ozmux/ozmux
```

This taps `not-elm/homebrew-ozmux`, installs `ozmux.app` into `/Applications`,
and pulls in `tmux` as a dependency. Upgrade later with:

```bash
brew upgrade --cask ozmux
```

The companion apps `ozmd` and `ozbrowser` (built with the `ratatui-ozma` SDK)
are installed from source with `just install-apps`.

## Features

### Webview

ozmux can display webviews inside the terminal, which opens up new
possibilities for TUI applications. For example:

- render rich graphics such as charts
- embed games built with WebAssembly
- host a local frontend (e.g. a dev server on localhost)

### Tmux Integration

ozmux supports tmux through its control mode (`tmux -CC`), so your existing
`tmux.conf` keybindings work as-is. ozmux starts as a plain single-pane
terminal; running `tmux -CC` inside it switches to integration mode, where
tmux windows and panes are rendered natively.

## CLI Tools

| name                                    | description            |
| --------------------------------------- | ---------------------- |
| [ozmd](./apps/ozmd/README.md)           | A rich markdown viewer |
| [ozbrowser](./apps/ozbrowser/README.md) | A tiny browser         |

## SDK

- [ratatui-ozma](sdk/ratatui-ozma) — Rust SDK: a ratatui widget and RPC
  handler for embedding ozmux webviews from a TUI app.
- [@ozma/web](sdk/ozma-web) — TypeScript client for the in-page `window.ozma`
  bridge.

## Ozma Webview Protocol

[docs/ozma_webview_protocol.md](docs/ozma_webview_protocol.md)

## Configuration

[docs/configs.md](docs/configs.md)

## License

MIT. See [LICENSE](LICENSE).
