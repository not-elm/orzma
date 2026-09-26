# Getting Started

> [!WARNING]
> orzma is in early development and may introduce breaking changes.

orzma is a terminal emulator that can render web pages inside the terminal. A
program running in an orzma pane can place a live web page among its own text
and exchange messages with it, so a terminal app can show rendered Markdown,
diagrams, or a website without leaving the terminal. orzma also splits its
window into panes, so you do not need a separate terminal multiplexer.

![orzma with a Markdown viewer and a browser in split panes](images/thumbnail.png)

## Supported platforms

- macOS 11 or later on Apple Silicon.
- Windows 10 version 1809 or later, and Windows 11 (x64).
- Linux is planned.

## Install

### macOS

Install orzma with Homebrew:

```sh
brew install --cask not-elm/orzma/orzma
```

This adds the `not-elm/homebrew-orzma` tap and installs `orzma.app` into
`/Applications`. To upgrade later, run:

```sh
brew upgrade --cask orzma
```

### Windows

Download `orzma-<version>-x64.msi` from the
[latest release](https://github.com/not-elm/orzma/releases/latest) and run it.
The installer is per-user: it needs no administrator prompt, installs into
`%LocalAppData%\Programs\orzma`, and puts `orzma`, `orzmd`, and `orzbrowser` on
your `PATH`.

Both installs include the [companion apps](companion-apps.md) `orzmd` and
`orzbrowser`. To build orzma from source, see
[CONTRIBUTING.md](https://github.com/not-elm/orzma/blob/main/CONTRIBUTING.md).

## First steps

Start orzma. It opens one pane that runs your shell.

Pane commands start with the *leader*: tap `Cmd` on macOS or `Alt` on Windows —
press and release it without any other key — and then press the command's key.

| Keys | Action |
| --- | --- |
| Leader, then `i` | Split the pane side by side. |
| Leader, then `o` | Split the pane top and bottom. |
| Leader, then `h` / `j` / `k` / `l` | Move to the pane on the left / below / above / on the right. |
| Leader, then `p` | Close the active pane. |

To see a web page inside the terminal, open a Markdown file with the bundled
viewer. In a directory that has a `README.md`, such as a project you have
cloned, run the command below, and press `q` to quit it:

```sh
orzmd README.md
```

orzma reads its settings from `~/.config/orzma/config.toml`
(`%USERPROFILE%\.config\orzma\config.toml` on Windows). The file is optional;
see [Configuration](configuration.md) for every setting.
