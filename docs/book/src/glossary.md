# Glossary

## Terminal

### Active pane

The pane that receives your keystrokes. A new pane and a pane you click
become the active pane, and the other panes are tinted (see
[Inactive panes](panes-and-workspaces.md#inactive-panes)).

### Alternate screen

A second screen that full-screen programs such as vim and less draw on. It has
no scrollback, and the [primary screen](#primary-screen) comes back unchanged
when the program exits.

### Chord

A key pressed together with zero or more modifiers, such as `Alt+h` or
`Cmd+Plus`. A *direct chord* runs an action by itself, and a *leader binding*
is the [leader](#leader) followed by one more key (see
[Chord syntax](key-bindings.md#chord-syntax)).

### Leader

A key that starts a two-key shortcut, written `<Leader>` in the configuration.
By default it is a tap of `Cmd` on macOS and of `Alt` on Windows and Linux
(see [The leader key](key-bindings.md#the-leader-key)).

### Pane

A part of the window that runs its own shell. The panes of a
[workspace](#workspace) share the window below the tab bar (see
[Panes and Workspaces](panes-and-workspaces.md)).

### Primary screen

The screen a shell prints to. Lines that scroll off its top go into the
[scrollback](#scrollback).

### Scrollback

The lines that have scrolled off the top of the primary screen. orzma keeps
the latest 10,000 lines of each pane. Scroll back with the mouse wheel or in
[vi mode](#vi-mode).

### Vi mode

A mode that moves a cursor over the screen and the scrollback with vi keys, to
select and copy text (see [Vi Mode](vi-mode.md)).

### Workspace

One layout of panes, shown as a tab in the tab bar at the top of the window.
The shells in the workspaces that are not on screen keep running (see
[Workspaces](panes-and-workspaces.md#workspaces)).

## Webview apps

These terms appear in [Building Webview Apps](building-webview-apps.md) and
the [Webview Protocol](protocol-reference.md).

### Bridge

The `window.orzma` object orzma adds to a page, through which the page calls
its program and exchanges events with it (see
[The `window.orzma` bridge](protocol-reference.md#the-windoworzma-bridge)).

### Control socket

The local socket a program talks to orzma over. orzma gives each pane the
socket's path in `ORZMA_SOCK` and a token in `ORZMA_TOKEN` (see
[The control socket](protocol-reference.md#the-control-socket)).

### Forward keys

Key chords that a program asks orzma to send to its pane instead of to its
focused page, so that the program keeps keys such as its own quit key (see
[Forward keys](protocol-reference.md#forward-keys)).

### Handle

The opaque ID orzma returns when a program registers content. It names the
[registration](#registration).

### Host

orzma's side of the protocol: it keeps each program's registrations and draws
their pages.

### Instance

The ID of one [placement](#placement). A program mounts, unmounts, focuses,
and navigates placements by their instance IDs (see
[Instance semantics](protocol-reference.md#instance-semantics)).

### Mount

To show a placement in a rectangle of terminal cells, with the APC `mount`
sequence or the socket `mount` op. Unmounting removes it again (see
[APC webview verbs](protocol-reference.md#apc-webview-verbs--mount--unmount)).

### Page

The web page a placement shows: the registered HTML, file, or URL, loaded in
its own [webview](#webview).

### Placement

One place where a registration's page appears in a pane. A registration can
have several placements, each with its own [instance](#instance) ID and its
own page.

### Program

The terminal app that runs in a pane, registers content, and mounts it. The
Rust SDK [`ratatui_orzma`](protocol-reference.md#sdks) implements the
program's side of the protocol.

### Registration

Content a program has registered with orzma: inline HTML, a directory of
files, or a URL. It is named by its [handle](#handle).

### Webview

The embedded browser that draws a page inside the terminal.
