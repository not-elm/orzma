# ozmux

A terminal multiplexer built as a single
[Bevy](https://bevyengine.org/) application (`ozmux-gui`). Terminal emulation,
GPU rendering, layout, input, and in-process CEF webview rendering all run in
one ECS world. A single Node host process is spawned for the (dormant) host-RPC
plumbing.

## Prerequisites

- Rust 1.95 (pinned by `rust-toolchain.toml`)
- Node ≥ 23.6 (the host runs as a single `node` process relying on native
  TypeScript type-stripping; dev/CI use Node 24) + `pnpm@10.30.2`
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
- `sdk/typescript` — `@ozmux/sdk` (the `./inline` OSC mount-sequence helper)
- `host/` — `@ozmux/host`, the single Node host runtime (bundled to `assets/host.mjs`)

## Inline webviews

A program in the shell can render a webview **inline in the terminal text flow**
(a live CEF webview composited in the terminal shader, scrolling with the text).
It registers content over the control plane to mint a handle, then writes an OSC
5379 `mount-inline;<handle>` sequence — the `@ozmux/sdk/inline` helper builds the
sequence:

```ts
import { mountInline } from '@ozmux/sdk/inline';
process.stdout.write('panel:\n');
process.stdout.write(mountInline(handle, { rows: 12, cols: 48 }));
```

For a runnable end-to-end client (register → mount → `window.ozmux` back-channel)
see [`examples/dyn_webview_client.rs`](examples/dyn_webview_client.rs). Click the
view to focus it; keys, wheel, and IME then route to the page; `Ctrl+Shift+Escape`
returns focus to the terminal. Full protocol, focus model, and limits:
[`docs/dyn-webview.md`](docs/dyn-webview.md) and
[`docs/inline-webview.md`](docs/inline-webview.md).

## Development

See `CLAUDE.md` for architecture and `.claude/rules/` for coding conventions.
