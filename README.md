# ozmux(Ozma Terminal Multiplexer)

> [!CAUTION]
> This app is still in early development and may introduce breaking changes.

ターミナル内にwebviewを描画することができるターミナルエミュレータ。
tmuxの統合機能も標準搭載しています。

## Installation

macOS (Apple Silicon) via Homebrew Cask:

```bash
brew install --cask not-elm/ozmux/ozmux
brew install ozmd ozbrowser # optional
```

This taps `not-elm/homebrew-ozmux`, installs `ozmux.app` into `/Applications`,
and pulls in `tmux` as a dependency. Upgrade later with:

```bash
brew upgrade --cask ozmux
```

## Features

### Webview

ターミナル内にWebviewを表示することができます。これは特にTUIアプリの可能性を広げることが期待できます。例えば:

- チャートのような高度なグラフィックをレンダリングする
- Wasmを使ったゲームを埋め込む
- ローカルホストのフロントエンド

### Tmux Intergration

TmuxのControl Modeを使ったtmux統合機能を標準でサポートしています。キーバインドはtmux.confに設定されているものをそのまま使うことができます。
`tmux -CC`をターミナル内で実行することでこのモードに切り替えることができます。

## SDK

- [ratatui_ozma](sdk/ratatui_zoma)

## Ozma Webview Protocol

Please see [docs/osc.md](docs/osc.md)

## Configs

Please see [docs/configs.md](docs/configs.md)

## Licenses

MIT
