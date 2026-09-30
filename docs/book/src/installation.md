# Installation

If you are upgrading from an earlier release, read [Upgrading](upgrading.md)
for the changes that need you to edit your configuration.

## Supported platforms

- macOS 11 or later on Apple Silicon.
- Windows 10 version 1809 or later, and Windows 11 (x64).
- Linux (x86_64) with glibc 2.35 or later. The `.deb` is tested on Ubuntu
  22.04 and 24.04.

## macOS

Install orzma with Homebrew:

```sh
brew install --cask not-elm/orzma/orzma
```

This adds the `not-elm/homebrew-orzma` tap and installs `orzma.app` into
`/Applications`. To upgrade later, run:

```sh
brew upgrade --cask orzma
```

To install without Homebrew, download `orzma-<version>-arm64.dmg` from the
[latest release](https://github.com/not-elm/orzma/releases/latest), open it, and
drag `orzma.app` onto the `Applications` folder next to it. This does not put
`orzmd` and `orzbrowser` on your `PATH`; to run them by name, add
`/Applications/orzma.app/Contents/Resources` to your `PATH`. To upgrade, download
the new dmg and replace `orzma.app` the same way.

### First launch

orzma is not notarized by Apple, so macOS blocks its first launch, however you
installed it, and again after each upgrade, with a warning that orzma could not
be verified as free of malware. To open it, clear its quarantine flag:

```sh
xattr -dr com.apple.quarantine /Applications/orzma.app
```

Alternatively, after macOS blocks orzma, open
**System Settings > Privacy & Security** (on macOS 11 and 12,
**System Preferences > Security & Privacy > General**) and click
**Open Anyway** next to the message about orzma.

## Windows

Download `orzma-<version>-x64.msi` from the
[latest release](https://github.com/not-elm/orzma/releases/latest) and run it.
The installer is per-user: it needs no administrator prompt, installs into
`%LocalAppData%\Programs\orzma`, and puts `orzma`, `orzmd`, and `orzbrowser` on
your `PATH`. To upgrade, run the installer of the new release; it replaces the
installed version.

## Linux

Use either the `.deb` or the tarball, not both: with both installed,
`~/.local/bin/orzma` usually shadows `/usr/bin/orzma`.

On Ubuntu, Debian, and other Debian-based distributions, download
`orzma_<version>_amd64.deb` from the
[latest release](https://github.com/not-elm/orzma/releases/latest) and install
it with apt, which also installs the system libraries orzma needs:

```sh
sudo apt install ./orzma_<version>_amd64.deb
```

The package puts `orzma`, `orzmd`, and `orzbrowser` in `/usr/bin`.

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
`~/.local/share/orzma` to your `PATH` to run them by name. You can also run
`./orzma` straight from the extracted directory without installing.

## Uninstall

Remove orzma the way you installed it:

- **Homebrew:** run `brew uninstall --cask orzma`.
- **dmg:** move `/Applications/orzma.app` to the Trash.
- **Windows:** open **Settings > Apps** (**Installed apps** on Windows 11),
  select **orzma**, and choose **Uninstall**. This also removes orzma from
  your `PATH` and the Start menu.
- **.deb:** run `sudo apt remove orzma`.
- **Tarball:** run `~/.local/share/orzma/uninstall.sh`
  (`$XDG_DATA_HOME/orzma/uninstall.sh` when `XDG_DATA_HOME` is set).

None of these removes your settings in `~/.config/orzma`
(`%USERPROFILE%\.config\orzma` on Windows); delete that directory to remove
them as well.

Every install includes the [companion apps](companion-apps.md) `orzmd` and
`orzbrowser`. To build orzma from source, see [Contributing](contributing.md).
