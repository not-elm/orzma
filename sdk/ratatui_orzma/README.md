# ratatui_orzma

A [ratatui](https://ratatui.rs) widget and RPC handler for embedding orzma
webviews in a terminal app.

Running inside an orzma pane, an app registers web content with orzma, draws it
with `WebviewWidget` in its ratatui layout, and exchanges calls and events with
the page. `OrzmaBackend` wraps the app's terminal backend and places each page
on every draw.

## Usage

```sh
cargo add ratatui_orzma ratatui@0.29
```

See [Building Webview Apps](https://not-elm.github.io/orzma/building-webview-apps.html)
for a tutorial, the [examples](examples) for complete programs, and
[docs.rs](https://docs.rs/ratatui_orzma) for the API.

## License

MIT. See [LICENSE](../../LICENSE).
