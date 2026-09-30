# Upgrading

This page lists, release by release, the changes that need you to edit your
configuration file and the changes in behavior you are likely to notice. When
you skip releases, apply each section from the oldest to the newest. The
[releases page](https://github.com/not-elm/orzma/releases) lists every change.

If orzma starts with the default settings after an upgrade, your configuration
file still uses something the new release rejects;
[Validation](configuration.md#validation) explains how to see the reason.

## 0.3.0

### The stock shortcuts are `Alt` chords

The pane and vi mode shortcuts moved from the leader to direct `Alt` chords,
and the new workspace shortcuts use `Alt` as well
([Default Key Bindings](default-key-bindings.md) lists them all):

| Action | 0.2 | 0.3 |
| --- | --- | --- |
| `enter-vi-mode` | `<Leader>s` | `Alt+s` |
| `select-left-pane`, `select-down-pane`, `select-up-pane`, `select-right-pane` | `<Leader>h`, `<Leader>j`, `<Leader>k`, `<Leader>l` | `Alt+h`, `Alt+j`, `Alt+k`, `Alt+l` |
| `split-vertical-pane`, `split-horizontal-pane` | `<Leader>i`, `<Leader>o` | `Alt+i`, `Alt+o` |
| `kill-pane` | `<Leader>p` | `Alt+p` |
| `resize-left-pane`, `resize-down-pane`, `resize-up-pane`, `resize-right-pane` | `<Leader:r>Shift+H`, `<Leader:r>Shift+J`, `<Leader:r>Shift+K`, `<Leader:r>Shift+L` | `r:Alt+Shift+H`, `r:Alt+Shift+J`, `r:Alt+Shift+K`, `r:Alt+Shift+L` |

- If your configuration binds one of the new `Alt` chords to another action,
  or uses one as a chord `leader`, orzma does not start. Rebind that action,
  or unbind the stock action that now holds the chord (see
  [Conflicts](key-bindings.md#conflicts-and-turning-the-leader-off)).
- The `Alt` chords orzma binds no longer reach the shell, such as readline's
  `Alt+c` and `Alt+r` (see
  [`Alt` chords, the shell, and the Option key](key-bindings.md#alt-chords-the-shell-and-the-option-key)).

To keep the 0.2 keys, bind them back:

```toml
[shortcuts]
enter-vi-mode = "<Leader>s"
select-left-pane = "<Leader>h"
select-down-pane = "<Leader>j"
select-up-pane = "<Leader>k"
select-right-pane = "<Leader>l"
split-vertical-pane = "<Leader>i"
split-horizontal-pane = "<Leader>o"
kill-pane = "<Leader>p"
resize-left-pane = "r:<Leader>Shift+H"
resize-down-pane = "r:<Leader>Shift+J"
resize-up-pane = "r:<Leader>Shift+K"
resize-right-pane = "r:<Leader>Shift+L"
```

### The right Option key is `Alt` on macOS

`[keyboard] option_as_alt` now defaults to `"right"` instead of `"none"`: the
right Option key runs the `Alt` shortcuts and sends Meta to the shell, and no
longer types special characters. The left Option key still does. If your
configuration sets `option_as_alt = "none"`, neither Option key runs the stock
shortcuts, so remove that line. A Japanese (JIS) keyboard has no right Option
key; set `"left"` or `"both"` there.

### Repeatable bindings are marked with `r:`

- `<Leader:r>x` is no longer accepted, and a configuration that uses it is
  ignored as a whole. Write `r:<Leader>x` instead.
- A direct chord now fires once per press, however long you hold it. Put `r:`
  in front of a chord you bound yourself to make it keep firing while held
  (see [Repeatable bindings](key-bindings.md#repeatable-bindings-r)).
- After a leader tap, a key with no `<Leader>` binding runs its direct chord
  instead of being swallowed.

### Shortcuts run while a web page has focus

Direct chords other than `copy` and `paste` now run orzma's shortcuts while a
web page has focus, and the page no longer receives them. To give them to the
page, set `direct-chords-over-webview = false` in `[shortcuts]` (see
[Shortcuts while a webview has focus](key-bindings.md#shortcuts-while-a-webview-has-focus)).

### The wheel scrolls more slowly

The `[mouse]` defaults changed: `lines_per_notch` from 3 to 1,
`cells_per_notch` from 0.5 to 0.3333, and `max_protocol_events_per_frame`
from 8 to 24. One line of wheel travel now scrolls about three lines instead
of six. To scroll the scrollback about as fast as before:

```toml
[mouse]
lines_per_notch = 3
cells_per_notch = 0.5
```

Two other changes cannot be undone by these settings. A program that tracks
the mouse now gets one wheel report per cell of travel. And at the new
defaults, `fine_lines` equals `lines_per_notch`, so holding `fine_modifier`
no longer slows scrolling until you raise `lines_per_notch`.

### Workspaces

orzma now keeps several layouts of panes as workspaces, listed in a tab bar at
the top of the window (see [Workspaces](panes-and-workspaces.md#workspaces)).
Closing the last pane of a workspace closes the workspace, and closing the
last workspace quits orzma.

### Companion apps

- orzmd: the page takes keyboard focus once it has loaded, and `Ctrl+c` no
  longer quits while it has focus; quit with `q`.
- orzbrowser: `Ctrl+c` no longer quits in Normal mode or in the address bar;
  quit with `q`. Text with a space, or a single word without a dot, is
  searched for instead of opened as an address, and `localhost`, an IP
  address, or `name:port` opens over `http` instead of `https`.

### Installing on macOS

The macOS download is a `.dmg` instead of a `.zip` (see
[Installation](installation.md#macos)).

## 0.2.0

### tmux integration removed

orzma no longer drives tmux: panes are orzma's own (see
[Panes and Workspaces](panes-and-workspaces.md)). These `[shortcuts]` actions
were removed: `detach-session`, `next-session`, `previous-session`,
`rename-session`, `new-window`, `kill-window`, `next-window`,
`previous-window`, `select-window-0` … `select-window-9`, `rename-window`, and
`zoom-pane`. The `[scrollback]` table was removed as well. A configuration
that still sets any of them is ignored as a whole, so delete those lines. The
workspace actions of 0.3.0 replace the window actions.

### Fonts are named, not loaded from files

Each face of `[font]` was a path to a font file; now it is a table that names
an installed font family and a style (see [`[font]`](configuration.md#font)).
A path makes orzma ignore the whole file, and so does an unknown key in
`[font]`.

```toml
[font.normal]
family = "JetBrains Mono"
style = "Regular"
```

### New shortcuts

`copy` (`Cmd+C`) and the font size shortcuts (`Cmd+Plus`, `Cmd+-`, and
`Cmd+0`) were added. If your configuration binds one of these chords to
another action, orzma does not start; rebind one of the two.

### Webview programs mount with an APC sequence

The `OSC 5379` sequences that mounted and unmounted a registration were
replaced by the APC `mount` and `unmount` sequences, which name an instance
with `n=` (see [mount](protocol-reference.md#mount)). The `focus` and
`navigate` ops name a placement by its `instance` instead of a `handle`. The
SDKs do this for you.
