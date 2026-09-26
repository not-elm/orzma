# Contributing to orzma

Thanks for helping! This guide covers the development setup, how the code is
organized, the conventions the code follows, how to send a change, and how to
work on the documentation.

## Development setup

orzma builds on macOS, on Windows 10 1809+ / 11 (x64), and on Linux (x86_64,
glibc 2.35 or later).

You need:

- Rust, installed with [rustup](https://rustup.rs). The repository pins the
  toolchain (1.95) in `rust-toolchain.toml`, and rustup installs it on first use.
- Node.js 22 or later, and pnpm 10.30.2 (the `packageManager` in
  `package.json`): `npm install -g pnpm@10.30.2`.
- [just](https://just.systems).
- On Windows, CMake and Ninja: `winget install Kitware.CMake Ninja-build.Ninja`.
- On Linux, the build packages the release build uses — on Ubuntu or Debian,
  `sudo apt install build-essential pkg-config libasound2-dev libudev-dev
  libwayland-dev libxkbcommon-dev libfontconfig1-dev` — and the runtime
  libraries that [Getting Started](https://not-elm.github.io/orzma/getting-started.html#linux)
  lists for the tarball.

Then, from the repository root:

```sh
pnpm install
just setup-cef   # one-time, macOS and Windows: installs the Chromium Embedded Framework and its render process
just run         # builds and runs orzma
```

Linux has no `just setup-cef`: the build downloads CEF and copies it beside the
binary.

| Command | What it does |
| --- | --- |
| `just build` | Build the workspace. |
| `just run` | Run orzma. |
| `just test` | Run every Rust test. |
| `pnpm -r test` | Run the TypeScript tests. |
| `pnpm check-types` | Type-check the TypeScript packages. |
| `just fix-lint` | Apply clippy fixes, rustfmt, and biome fixes. |
| `just install-apps` | Build and install `orzmd` and `orzbrowser`. |

On Windows, `just build`, `just run`, and `just test` set `CEF_PATH` for you.
When you run cargo yourself — including through `just fix-lint` — set
`CEF_PATH` to `%USERPROFILE%\.cache\orzma\cef` first.

## Architecture

orzma is a single native GUI application. Terminal emulation, GPU rendering,
the multiplexer, input, and webview rendering all run in one
[Bevy](https://bevyengine.org) app; there is no daemon, HTTP server, or
browser-side frontend.

```mermaid
flowchart TD
    orzma["orzma (the app)"]
    webview["bevy_orzma_webview"]
    host["bevy_orzma_webview_host"]
    renderer["bevy_orzma_tty_renderer"]
    bevy_orzmux["bevy_orzmux"]
    orzmux["orzmux"]
    tty["orzma_tty"]
    vt["orzma_vt"]
    configs["orzma_configs"]
    cef["bevy_cef (crates.io)"]
    cefcore["bevy_cef_core (crates.io)"]
    sdk["ratatui_orzma (SDK)"]
    orzmd["orzmd"]
    orzbrowser["orzbrowser"]
    orzma --> webview
    orzma --> configs
    webview --> renderer
    webview --> host
    webview --> cef
    renderer --> bevy_orzmux
    bevy_orzmux --> orzmux
    orzmux --> tty
    tty --> vt
    host --> cefcore
    orzmd --> sdk
    orzbrowser --> sdk
```

Arrows point from a crate to the crates it uses; a dependency already implied by
a path through other crates is left out.

| Crate | Role |
| --- | --- |
| `orzma` (`src/`) | The app: window, input, UI, and the plugins below. |
| `orzma_vt` | Terminal emulation: the screen, scrollback, escape sequences, and selection. |
| `orzma_tty` | Runs the shell under a PTY and drives `orzma_vt`. |
| `orzmux` | The multiplexer backend: a thread that owns every pane and the pane layout. |
| `bevy_orzmux` | Connects the backend to the Bevy app. |
| `bevy_orzma_tty_renderer` | Draws the terminal grid on the GPU. |
| `bevy_orzma_webview` | Webviews: the control socket, mounting, and the `window.orzma` bridge. |
| `bevy_orzma_webview_host` | Serves webview content through the `orzma://` scheme. |
| `orzma_configs` | Loads `config.toml`. |
| `ratatui_orzma` (`sdk/`) | The Rust SDK for webview apps. `@orzma/web` is its page-side companion. |

At run time, the `orzmux` thread owns each pane's PTY and terminal state; the app
sends it commands and receives layout snapshots and frames, which
`bevy_orzma_tty_renderer` draws. Programs in a pane reach webviews through the
[webview protocol](https://not-elm.github.io/orzma/protocol-reference.html).

## Conventions

- Code comments are written in English.
- Rust code follows
  [.claude/rules/rust.md](https://github.com/not-elm/orzma/blob/main/.claude/rules/rust.md),
  and TypeScript follows
  [.claude/rules/typescript.md](https://github.com/not-elm/orzma/blob/main/.claude/rules/typescript.md).
  They cover the comment rules, doc comments, imports, and error handling.
- Run `just fix-lint` before sending a change. CI runs `cargo fmt --check`,
  `cargo clippy` with warnings as errors, the tests, `cargo doc`, cargo-deny,
  the third-party license check, and `pnpm lint:ci`.
- Commit messages follow the Conventional Commits style of the history, for
  example `feat(webview): …`, `fix(input): …`, or `docs: …`.

## Pull requests

Open a pull request against `main` and fill in the template. Release notes are
grouped by label, so add one of `breaking-change`, `enhancement`,
`performance`, `bug`, `documentation`, or `dependencies` — or `skip-changelog`
to leave the change out of the notes.

## Documentation

The user guide lives in `docs/book` and is built with
[mdBook](https://rust-lang.github.io/mdBook/). It is published to
<https://not-elm.github.io/orzma/> when a release tag is pushed.

```sh
just setup-book   # one-time: installs the pinned mdBook and mdbook-mermaid
just book-serve   # preview with live reload
just book         # build into target/book
```

- When a change affects what users see — a configuration key, a shortcut, or
  the webview protocol — update the matching page under `docs/book/src` in the
  same pull request.
- Draw diagrams with [Mermaid](https://mermaid.js.org) in a code block tagged
  `mermaid`. In a `sequenceDiagram`, write a `;` inside a message as `#59;`.
- Link to repository files from the book with absolute URLs such as
  `https://github.com/not-elm/orzma/blob/main/...`; the published book cannot
  follow relative links out of `docs/book`.
- To upgrade mdBook or mdbook-mermaid, change the versions in the `justfile`.
  After upgrading mdbook-mermaid, delete `docs/book/mermaid.min.js` and
  `docs/book/mermaid-init.js` and run `mdbook-mermaid install docs/book`,
  because it does not overwrite existing files. The lychee version is pinned in
  `.github/workflows/book.yml`.
- When the SDK moves to a new ratatui version, update the version in the Setup
  section of `docs/book/src/building-webview-apps.md` and in
  `sdk/ratatui_orzma/README.md`.
