# ratatui-orzma

Ratatui backend for the orzma in-process webview bridge.

Implements a [`ratatui`](https://ratatui.rs) backend that renders terminal UI inside an orzma
webview pane. Applications draw to the backend as they would with any ratatui terminal, and the
output is forwarded to the orzma host via the OSC-based control protocol.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
ratatui-orzma = "0.1"
```

## License

MIT
