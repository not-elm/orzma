# First Steps

This walkthrough covers the keys you need every day, using the default key
bindings. [Default Key Bindings](default-key-bindings.md) lists all of them.

## The Alt key on macOS

On macOS, the right Option key acts as `Alt` for orzma's shortcuts, and the
left Option key keeps typing special characters. If your Mac has no right
Option key, as with a Japanese (JIS) keyboard, set
[`option_as_alt`](configuration.md#keyboard-option_as_alt) in your
configuration file (see [Change a setting](#change-a-setting)):

```toml
[keyboard]
option_as_alt = "left"
```

On Windows and Linux, either `Alt` key works, except AltGr on a keyboard
layout that has it.

## Split the window

1. Press `Alt+i` to split the pane side by side, or `Alt+o` to split it top
   and bottom. The new pane becomes the active pane.
2. Press `Alt+h`, `Alt+j`, `Alt+k`, or `Alt+l` to move to the pane on the
   left, below, above, or on the right. You can also click a pane.
3. Hold `Alt+Shift+H`, `J`, `K`, or `L` to move a divider, or drag it with the
   mouse.
4. Press `Alt+p` to close the active pane.

[Panes and Workspaces](panes-and-workspaces.md) has the details.

## Open a workspace

A workspace is a separate layout of panes, shown as a tab at the top of the
window.

1. Press `Alt+c` to open a new workspace.
2. Press `Alt+1` to go back to the first one, or `Alt+]` and `Alt+[` to step
   through them. You can also click a tab.
3. Press `Alt+r` to rename the workspace on screen.

## Copy text in vi mode

1. Press `Alt+s` to enter vi mode.
2. Move with `h`, `j`, `k`, and `l`, press `v` to start a selection, and move
   again to extend it.
3. Press `y` to copy the selection and leave vi mode.

[Vi Mode](vi-mode.md) lists every motion.

## Read a Markdown file

Run orzmd with a Markdown file:

```sh
orzmd README.md
```

orzmd shows the rendered file in the pane and updates it when the file is
saved. Press `j` and `k` to scroll, and `q` to quit. If your shell cannot find
`orzmd`, see [Companion Apps](companion-apps.md) to add it to your `PATH`.

## Change a setting

orzma reads its configuration file when it starts. Unless you set
`ORZMA_CONFIG` or `XDG_CONFIG_HOME`, the file is `~/.config/orzma/config.toml`
(`%USERPROFILE%\.config\orzma\config.toml` on Windows). Create the file, add
a setting, and restart orzma:

```toml
[font]
size = 14
```

If the setting has no effect, check that the file is where
[File location](configuration.md#file-location) says; if orzma reports a
problem with it, [Validation](configuration.md#validation) explains how to
see the reason.
[Configuration](configuration.md) lists every setting, and
[Key Bindings](key-bindings.md) explains how to change the shortcuts.
