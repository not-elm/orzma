# orzma

> [!WARNING]
> orzma is in early development and may introduce breaking changes.

orzma is a terminal emulator that can render web pages inside the terminal. A
program running in an orzma pane can place a live web page among its own text
and exchange messages with it.

![A program placing a live web page in an orzma pane](./docs/book/src/images/thumbnail.gif)

## Companion apps

The [webview protocol](https://not-elm.github.io/orzma/protocol-reference.html)
extends what a terminal program can do. Every install includes two apps built
on it, orzmd (left) and orzbrowser (right), and each runs inside an orzma pane.

![orzmd rendering a Markdown file beside orzbrowser showing a website](./docs/book/src/images/thumbnail2.png)

| Name                                                          | Description               |
| ------------------------------------------------------------- | ------------------------- |
| [orzmd](https://not-elm.github.io/orzma/orzmd.html)           | A rich Markdown viewer    |
| [orzbrowser](https://not-elm.github.io/orzma/orzbrowser.html) | A keyboard-driven browser |

To build your own, see [SDK](#sdk).

## Documentation

The user guide is published at <https://not-elm.github.io/orzma/> with each
release. Its sources are in [`docs/book/src`](docs/book/src).

- [Installation](https://not-elm.github.io/orzma/installation.html) — install orzma.
- [Configuration](https://not-elm.github.io/orzma/configuration.html),
  [Key Bindings](https://not-elm.github.io/orzma/key-bindings.html), and
  [Default Key Bindings](https://not-elm.github.io/orzma/default-key-bindings.html).
- [Building Webview Apps](https://not-elm.github.io/orzma/building-webview-apps.html)
  and the [Webview Protocol](https://not-elm.github.io/orzma/protocol-reference.html).

## Installation

macOS 11 or later on Apple Silicon, with Homebrew:

```bash
brew install --cask not-elm/orzma/orzma
```

Or download `orzma-<version>-arm64.dmg` from the
[latest release](https://github.com/not-elm/orzma/releases/latest), open it, and drag
`orzma.app` into `Applications`. orzma is not notarized, so see
[First launch](https://not-elm.github.io/orzma/installation.html#first-launch)
before opening it.

Windows 10 1809 or later, and Windows 11 (x64): download `orzma-<version>-x64.msi` from the
[latest release](https://github.com/not-elm/orzma/releases/latest) and run it.

Linux (x86_64) with glibc 2.35 or later: download the `.deb` for Ubuntu or
Debian, or the tarball for other distributions, from the
[latest release](https://github.com/not-elm/orzma/releases/latest).

See [Installation](https://not-elm.github.io/orzma/installation.html) for details.

## SDK

- [ratatui_orzma](sdk/ratatui_orzma) — Rust SDK: a ratatui widget and RPC
  handler for embedding orzma webviews in a TUI app.
- [@orzma/web](sdk/orzma-web) — TypeScript client for the in-page
  `window.orzma` bridge.

See [Building Webview Apps](https://not-elm.github.io/orzma/building-webview-apps.html)
for a tutorial.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE).
