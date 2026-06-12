# ozmux

A terminal multiplexer built as a single
[Bevy](https://bevyengine.org/) application (`ozmux-gui`). Terminal emulation,
GPU rendering, layout, input, and in-process CEF webview rendering all run in
one ECS world. Node extensions (e.g. `@memo`) are spawned as child processes.

## Prerequisites

- Rust 1.95 (pinned by `rust-toolchain.toml`)
- Node ≥ 23.6 (the host runs extensions via `node bootstrap.ts`, relying on
  native TypeScript type-stripping; dev/CI use Node 24) + `pnpm@10.30.2`
- The Chromium Embedded Framework, installed once:
  ```bash
  make setup-cef
  ```

## Run

```bash
pnpm install            # link @ozmux/sdk into extensions
cargo run               # or: make run
```

## Layout

- `src/` — the `ozmux-gui` Bevy binary
- `crates/` — `bevy_terminal`, `bevy_terminal_renderer`, `extension_host`, `multiplexer`, `configs`
- `sdk/typescript` — `@ozmux/sdk` (consumed by extensions)
- `extensions/memo` — the `@memo` Node extension

## Development

See `CLAUDE.md` for architecture and `.claude/rules/` for coding conventions.
