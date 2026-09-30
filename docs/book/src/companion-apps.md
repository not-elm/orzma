# Companion Apps

orzma comes with two terminal apps that show web pages inside the terminal,
built with the `ratatui_orzma` SDK: **orzmd**, a Markdown viewer, and
**orzbrowser**, a keyboard-driven browser. Homebrew links both into your `PATH`, and the
Windows installer and the Linux `.deb` add them to it. The macOS dmg keeps them
inside `orzma.app`, and the Linux tarball keeps them in `~/.local/share/orzma`;
see [Installation](installation.md) to add them to your `PATH`.
To build them from a clone of the repository instead, run `just install-apps`.

- [orzmd](orzmd.md) is a Markdown viewer with live reload, diagrams, math,
  and in-page search.
- [orzbrowser](orzbrowser.md) is a keyboard-driven browser with link hints and
  an address bar that also searches.

Both run only inside an orzma pane. Anywhere else, they exit with an error
such as:

```text
orzmd: not inside an orzma pane: ORZMA_SOCK is unset. Run orzmd inside an orzma pane.
```
