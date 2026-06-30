# ratatui-ozma

Ratatui backend for the ozma in-process webview bridge.

Implements a [`ratatui`](https://ratatui.rs) backend that renders terminal UI inside an ozma
webview pane. Applications draw to the backend as they would with any ratatui terminal, and the
output is forwarded to the ozma host via the OSC-based control protocol.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
ratatui-ozma = "0.1"
```

## License

MIT
