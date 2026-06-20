# ozmux

A terminal multiplexer built as a single
[Bevy](https://bevyengine.org/) application (`ozmux-gui`). Terminal emulation,
GPU rendering, layout, input, and in-process CEF webview rendering all run in
one ECS world.

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

## Prerequisites

- Rust 1.95 (pinned by `rust-toolchain.toml`)
- Node + `pnpm@10.30.2` (for the `@ozma/web` TypeScript package; dev/CI use Node 24)
- The Chromium Embedded Framework, installed once:
  ```bash
  make setup-cef
  ```

## Run

```bash
pnpm install
cargo run               # or: make run
```

## Layout

- `src/` — the `ozmux-gui` Bevy binary
- `crates/` — `ozma_tty_engine`, `ozma_tty_renderer`, `extension_host`, `multiplexer`, `configs`
- `sdk/ozma-web` — `@ozma/web` (in-page `window.ozma` bridge client for webview pages)

## Webviews

A program in the shell can render a webview **inline in the terminal text flow**
(a live CEF webview composited in the terminal shader, scrolling with the text).
It registers content over the control plane to mint a handle, then writes an OSC
5379 `mount;<handle>` sequence:

```sh
printf '\033]5379;mount;%s;12;48\033\\' "$handle"
printf '\n%.0s' $(seq 12)
```

For a runnable end-to-end client (register → mount → `window.ozma` back-channel)
see [`examples/dyn_webview_client.rs`](examples/dyn_webview_client.rs). Click the
view to focus it; keys, wheel, and IME then route to the page; `Ctrl+Shift+Escape`
returns focus to the terminal. Full protocol, focus model, and limits:
[`docs/dyn-webview.md`](docs/dyn-webview.md) and
[`docs/webview.md`](docs/webview.md).

## Development

See `CLAUDE.md` for architecture and `.claude/rules/` for coding conventions.
