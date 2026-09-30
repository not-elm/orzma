# Key Bindings

orzma reads key bindings from two tables of the
[configuration file](configuration.md):

- `[shortcuts]` holds orzma's own shortcuts. Every section below up to
  [Example](#example) describes it.
- `[vi-mode]` holds the keys that work inside [vi mode](vi-mode.md).
  [Vi mode keys](#vi-mode-keys-vi-mode) describes it.

The keys orzma binds out of the box are listed in
[Default Key Bindings](default-key-bindings.md).

Each `[shortcuts]` action takes one binding: a direct chord such as `Alt+h`, a
`<Leader>` binding such as `<Leader>u`, either of them with `r:` in front, or
`""` to unbind it. An action you leave out keeps its default.

## Chord syntax

A chord is zero or more modifiers followed by exactly one key, joined with `+`.

- **Modifiers** (case-insensitive): `Cmd` (also `Command` / `Meta` / `Super`),
  `Ctrl`, `Shift`, `Alt` (also `Opt` / `Option`).
- **Keys**: a letter or a digit (letters are case-insensitive), `[`, `]`, `-`,
  `=`, or a named key: `Escape` `Space` `Enter` `Tab` `Backspace` `ArrowUp`
  `ArrowDown` `ArrowLeft` `ArrowRight` `Plus`. Named keys are case-sensitive,
  so `escape` is not one. Any other single character is accepted for an
  action but never fires, and orzma logs a warning; as a chord `leader`, it
  stops orzma from starting.
- For the `+` key itself, use the token `Plus` (e.g. `Cmd+Plus`).

Examples: `Cmd+Shift+Q`, `Ctrl+Alt+ArrowLeft`, `Cmd+Plus`.

Invalid chords — an empty token (`Cmd+`), an unknown or miscapitalized named
key (`Cmd+F12`, `Cmd+escape`), a duplicated modifier (`Cmd+Meta+S`), or more
than one key (`Cmd+S+T`) — make orzma ignore the whole file and start with the
defaults (see [Validation](configuration.md#validation)).

## The leader key

The *leader* is a second way to bind an action: the leader followed by one
more key, written `<Leader>` in the configuration. By default only
`release-webview-focus` (`<Leader>u`) uses it; the other stock shortcuts are
direct chords such as `Alt+h`. The leader is a tap of a modifier: `Cmd` on
macOS and `Alt` on Windows and Linux. Press and release the modifier with no
other key or mouse button in between, then press the action's key.

After a tap, the next keystroke runs a `<Leader>` action when one matches.
Otherwise it is handled as if no leader had been tapped when it has a direct
chord, and is swallowed when it has none. The leader does not time out while
it waits, even if you switch to another window and back. Switching windows
before you release the modifier cancels the tap.

- `leader` sets the leader: a modifier to tap (`"Cmd"`, `"Ctrl"`, or
  `"Alt"`, or one of their aliases above), a chord such as `"Ctrl+A"` (press
  the chord, then the action's key), or `""` to turn the leader off.
  `"Shift"` is not allowed and makes orzma ignore the whole file. The leader
  does nothing while no action has a `<Leader>` binding.
- `leader-tap-timeout-ms` is how long a tap may last before it no longer
  counts, 300 ms by default; `0` reverts to 300.
- `repeat-time-ms` is how long a `r:<Leader>` binding (below) waits for its
  key again, 500 ms by default; `0` turns repeating off for them. It does not
  affect direct chords marked `r:`.

## Repeatable bindings (`r:`)

Put `r:` in front of a binding to make it repeatable. It works on both kinds of
binding:

- **A direct chord** such as `r:Alt+Shift+H` keeps firing while you hold it,
  at your system's key repeat rate. A direct chord without `r:` fires once per
  press, however long you hold it.
- **A leader binding** such as `r:<Leader>Shift+H` re-fires without the
  leader: after it fires, pressing any repeat-marked key again within
  `repeat-time-ms` (default 500) runs its action again, and each fire starts
  that time over. Holding the key down keeps firing. Any other key —
  including keys bound with plain `<Leader>` — ends the repeat at once and is
  handled normally (it is never swallowed). Pressing the leader during the
  repeat starts a fresh leader sequence.

The stock resize bindings, `increase-font-size`, and `decrease-font-size` carry
`r:`; no other stock binding does.

Caveat: with a letter key (say `r:<Leader>h`), typing that same letter into the
shell before `repeat-time-ms` runs out re-fires the action instead of reaching
the terminal. If that bites, set `repeat-time-ms = 0` or drop the `r:` from
that binding.

In vi mode a repeatable leader binding fires only on the key pressed right
after the leader: the repeat ends on the next key event, and holding the key
does not keep firing. A second press or an auto-repeat is read as a
`[vi-mode]` key instead. A repeatable direct chord, such as the stock
`r:Alt+Shift+H`, runs before the `[vi-mode]` keys and keeps firing while held.

## `Alt` chords, the shell, and the Option key

The stock pane, workspace, and vi-mode shortcuts are `Alt` chords, so those
keys no longer reach the program in the terminal as meta-prefixed keys:
readline's `Alt+c` (capitalize word), `Alt+r` (revert line), and `Alt+1` …
`Alt+9` (numeric argument), for example. Unbind or rebind a shortcut to give
its key back to the shell. `Alt` chords that orzma does not bind, such as
`Alt+b` and `Alt+f`, still reach the shell.

- **macOS:** the Option key that `option_as_alt` makes Alt runs the `Alt`
  chords — the right Option key by default (see
  [`option_as_alt`](configuration.md#keyboard-option_as_alt)). The other Option key types
  special characters, so left `Option+i` starts an accent instead of splitting
  the pane; it still counts as `Alt` with an arrow or another key that types
  no character, and together with `Ctrl` or `Cmd`. A MacBook with a Japanese
  (JIS) keyboard has no right Option key; set `option_as_alt = "left"` or
  `"both"` there.
- **Windows and Linux:** on a keyboard layout with AltGr, such as German, the
  right Alt key is AltGr and types characters; use the left Alt key for the
  shortcuts.

## Conflicts and turning the leader off

Things to know when you rebind:

- **Rebinding a chord that a stock default already uses** (e.g.
  `split-vertical-pane = "Alt+h"`, which collides with the default
  `select-left-pane = "Alt+h"`) is a startup validation error naming both
  actions. Unbind the stock default explicitly (`select-left-pane = ""`) or
  pick a free chord.
- **The stock pane, workspace, and vi-mode shortcuts are `Alt` chords** (see
  [Default Key Bindings](default-key-bindings.md)). If your configuration
  binds one of those chords to another action, or uses one of them as a chord
  `leader`, orzma does not start (see
  [Validation](configuration.md#validation)). Rebind that action to a free
  chord, or unbind the stock action that takes the chord, e.g.
  `rename-workspace = ""`. Binding a chord to the action that already has it by
  default is not a conflict. A direct chord and a `<Leader>` binding never
  collide, even on the same key, such as `s` and `<Leader>s`.
- **Two spellings of one key are not caught at startup.** `Cmd+Plus` and
  `Cmd+=` press the same key: orzma starts, logs a warning, and runs only one
  of the two actions. Bind each key once.
- **`leader = ""` disables every `<Leader>`-bound action at once** — with the
  stock defaults that is only `release-webview-focus`, silently (a warning is
  logged, but startup succeeds). If you disable the leader, rebind it to a
  direct chord, e.g. `release-webview-focus = "Ctrl+Shift+U"`.

## Shortcuts while a webview has focus

Once you click into a webview, keys go to the page, but orzma's own shortcuts
still come first:

<!-- ANCHOR: webview-focus -->

- `<Leader>` bindings and `release-webview-focus` always run.
- Other direct chords, such as `Cmd+Plus` or the stock `Alt` pane and
  workspace chords, run while
  [`direct-chords-over-webview`](configuration.md#shortcuts-direct-chords-over-webview)
  is `true`, the default. Set it to `false` to let
  the page have them; `release-webview-focus` (`<Leader>u`) still takes the
  keyboard back.
- `copy` and `paste` bound to direct chords never run while a webview has
  focus, so `Cmd+C` and `Cmd+V` copy and paste inside the page.

<!-- ANCHOR_END: webview-focus -->

A chord that runs an orzma shortcut never reaches the page, and it runs even
when the page's program lists it as a forward key.

## The `+` and `-` keys

`Ctrl++` is not a valid value: a chord is split on `+`, so write `Ctrl+Plus`.
A `Plus` binding also fires with Shift held, since `+` is Shift+`=` on a US
layout. Key bindings match physical key positions, so on a non-US layout the
`Plus` and `-` positions may not be where the labels are. The numeric keypad's
`+` and `-` are not bindable. `Plus` resolves to the physical position of the
`=` key on a US layout, and fires whether or not Shift is held — including
when it is the leader. On a layout with a dedicated `+` key, such as German,
that position is a different key, so bind the key you actually want by name
instead. `[` and `]` name physical key positions of a US layout; on a JIS
keyboard they are the keys labelled `@` and `[`.

## Example

A `[shortcuts]` table that changes a few defaults and keeps the rest:

```toml
[shortcuts]
# Tap Ctrl to start a <Leader> binding.
leader = "Ctrl"
# Split with the leader, giving Alt+i and Alt+o back to the shell.
split-vertical-pane   = "<Leader>i"
split-horizontal-pane = "<Leader>o"
# Give Alt+r back to the shell.
rename-workspace      = ""
# Keep resizing while Shift+H is held or pressed again within repeat-time-ms.
resize-left-pane      = "r:<Leader>Shift+H"
```

## Vi mode keys (`[vi-mode]`)

The `[vi-mode]` actions and their default keys are listed under
[Vi mode](default-key-bindings.md#vi-mode).

A `[vi-mode]` entry is an optional `Ctrl+` prefix plus exactly one key.

- **Keys** are either a single character, matched **case-sensitively**
  (`"w"` and `"W"` are different bindings — Shift is expressed through the
  character's case, e.g. `"W"` means Shift+w, not `"Shift+w"`), or one of the
  named keys `Escape` `Enter` `Space` `Tab` `Backspace` `ArrowUp` `ArrowDown`
  `ArrowLeft` `ArrowRight`.
- **`Ctrl+` is the only modifier prefix accepted.** `Cmd+`, `Alt+`, `Shift+`
  (and their aliases) are parse errors inside `[vi-mode]` — Shift is
  expressed via character case as above, and Cmd/Alt chords are reserved for
  application shortcuts (`[shortcuts]`); vi mode does not match keystrokes with Cmd or Alt held.
- After `Ctrl+`, the key must be an ASCII alphanumeric character or a named
  key — `Ctrl+$` is a parse error. `Ctrl+` entries match on the physical key
  pressed (not the character it produces), so they behave the same regardless
  of layout or case, and they do not match while Shift is also held.
- **Values** are a single key string (`yank = "Y"`) or an array of key
  strings (`exit = ["q", "Escape", "Ctrl+C"]`); any action can be bound to
  zero, one, or several keys.
- **`""` or `[]` unbinds** an action (for example, `search-forward = ""`).
  An array that holds an empty string, such as `[""]`, makes orzma ignore
  the whole file.
- **Duplicate keys are a startup error**: if the same key, however it is
  spelled (`"Space"` and `" "`, `"Ctrl+F"` and `"ctrl+f"`), is bound more than
  once in `[vi-mode]`, orzma fails at startup naming every colliding action
  (as duplicate chords are in `[shortcuts]`). An unknown
  action name, like the parse errors above, makes orzma ignore the whole
  file instead (see [Validation](configuration.md#validation)).

Shadowing note: `[shortcuts]` chords (both leader-scoped and direct) are
matched **before** `[vi-mode]` keys. If the same keystroke is bound in both
tables, the `[shortcuts]` action normally fires and the `[vi-mode]` binding
never sees it — e.g. setting `leader = "Ctrl+B"` shadows the default
`page-up = "Ctrl+B"` binding while vi mode is active. orzma does not
validate across the two tables; check your own bindings for overlap.

Two actions decline the keystroke instead of shadowing it, so the `[vi-mode]`
binding still runs:

- A direct `paste` chord does nothing in vi mode, so it passes through — the
  stock `Ctrl+V` on Windows and Linux reaches `toggle-rect-selection`. A
  `<Leader>`-scoped `paste` binding still pastes.
- `copy` passes a Ctrl-only chord through whenever there is no selection to
  copy — the stock `Ctrl+C` on Windows and Linux reaches `exit`. With a
  selection, it copies and clears the selection without leaving vi mode.
