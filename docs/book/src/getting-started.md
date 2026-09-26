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
- Linux (x86_64) with glibc 2.35 or later. The `.deb` is tested on Ubuntu
  22.04 and 24.04.

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

### Linux

Use either the `.deb` or the tarball, not both: with both installed,
`~/.local/bin/orzma` usually shadows `/usr/bin/orzma`.

On Ubuntu, Debian, and other Debian-based distributions, download
`orzma_<version>_amd64.deb` from the
[latest release](https://github.com/not-elm/orzma/releases/latest) and install
it with apt, which also installs the system libraries orzma needs:

```sh
sudo apt install ./orzma_<version>_amd64.deb
```

The package puts `orzma`, `orzmd`, and `orzbrowser` in `/usr/bin`. To uninstall,
run `sudo apt remove orzma`; your settings in `~/.config/orzma` are kept.

On other distributions, first install the system libraries that orzma and its
embedded Chromium need. The package names below are Ubuntu's and Debian's, so
install your distribution's equivalents; on Ubuntu 24.04 or later and Debian 13
or later, `libasound2` is named `libasound2t64`.

```sh
sudo apt install libnss3 libnspr4 libatk1.0-0 libatk-bridge2.0-0 libcups2 libdrm2 \
  libgbm1 libxkbcommon0 libxcomposite1 libxdamage1 libxrandr2 libxfixes3 \
  libpango-1.0-0 libcairo2 libgtk-3-0 libasound2 libdbus-1-3 libglib2.0-0 \
  libudev1 libwayland-client0 libfontconfig1 \
  libx11-6 libx11-xcb1 libxcursor1 libxi6 libxkbcommon-x11-0 libvulkan1 libegl1
```

Then download `orzma-<version>-x86_64-linux.tar.gz` from the
[latest release](https://github.com/not-elm/orzma/releases/latest) and run its
installer:

```sh
tar xzf orzma-<version>-x86_64-linux.tar.gz
cd orzma-<version>-x86_64-linux
./install.sh
```

The installer is per-user and needs no root. It copies orzma into
`~/.local/share/orzma` (`$XDG_DATA_HOME/orzma` when `XDG_DATA_HOME` is set),
links `~/.local/bin/orzma`, and adds orzma to your desktop's application list.
It does not put `orzmd` and `orzbrowser` on your `PATH`; add
`~/.local/share/orzma` to your `PATH` to run them by name. You can also run `./orzma` straight from the
extracted directory without installing. To uninstall, run
`~/.local/share/orzma/uninstall.sh`; your settings in `~/.config/orzma` are
kept.

Every install includes the [companion apps](companion-apps.md) `orzmd` and
`orzbrowser`. To build orzma from source, see
[CONTRIBUTING.md](https://github.com/not-elm/orzma/blob/main/CONTRIBUTING.md).

## First steps

Start orzma. It opens one pane that runs your shell.

Pane commands start with the *leader*: tap `Cmd` on macOS or `Alt` on Windows
and Linux — press and release it without any other key — and then press the
command's key.

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
