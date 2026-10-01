# Terminal Compatibility

This page lists what a program running in orzma can rely on: the environment
it starts in, the keys and mouse events it receives, and the escape sequences
orzma understands.

## Environment

- `TERM` comes from the environment orzma was started in, so panes of an orzma
  started from another terminal inherit that terminal's `TERM`. When `TERM` is
  unset or empty, as when you open orzma from the Dock or a desktop menu, orzma
  sets `TERM=xterm-256color` and `COLORTERM=truecolor`. orzma has no terminfo
  entry of its own.
- On macOS, when the locale (`LC_ALL`, `LC_CTYPE`, or `LANG`) is not UTF-8,
  orzma sets `LC_CTYPE=en_US.UTF-8`.
- Every pane gets `ORZMA_SOCK` and `ORZMA_TOKEN`, which webview apps use to
  reach orzma (see [Discovery](protocol-reference.md#discovery)).
- orzma does not set `TERM_PROGRAM`. Panes of an orzma started from another
  terminal inherit that terminal's `TERM_PROGRAM` and similar variables, so
  programs may treat orzma as that terminal.

## Keyboard

- With a key that types a character, `Alt`, including the Option key that
  [`option_as_alt`](configuration.md#keyboard-option_as_alt) makes `Alt` on
  macOS, sends `ESC` before the character.
- `Ctrl` with a letter sends that letter's control character. `Ctrl` with any
  other key, such as `Space`, `[`, `\`, or `/`, sends the plain character, and
  `Ctrl+Alt` with a letter sends only the control character.
- `Alt` and `Ctrl` do not change `Backspace`, `Enter`, `Escape`, or `Tab`;
  `Shift+Tab` sends `CSI Z`.
- The arrow keys, `Home`, and `End` follow the application cursor keys mode
  (DECCKM).
- Modifiers are not encoded for the arrow, navigation, and editing keys:
  `Ctrl+ArrowLeft` sends the same bytes as `ArrowLeft`.
- `F1` to `F12` send nothing.

## Mouse

- Mouse tracking: modes 1000 (clicks), 1002 (drags), and 1003 (all motion),
  reported in the X10 encoding or the SGR encoding (1006). The X10 encoding
  reports a column or row past 223 as 223. The UTF-8 (1005) and urxvt (1015)
  encodings are not supported.
- Hold `Shift` to keep the mouse from a program that tracks it, so you can
  select text; the wheel then acts as it does for a program that does not
  track the mouse.
- On the alternate screen, the wheel sends arrow keys to a program that does
  not track the mouse (alternate scroll, mode 1007, on by default).
- Focus reporting (mode 1004) sends `CSI I` and `CSI O` when the pane becomes
  or stops being the active pane, and when the orzma window gains or loses
  focus. A web page in the pane taking the keyboard sends no report.

## OSC sequences

| OSC | What it does | Notes |
| --- | --- | --- |
| 0, 2 | Set the window title | The window shows the active pane's title. `CSI 22 t` and `CSI 23 t` save and restore the title. orzma does not report the title back, and ignores OSC 1 (icon name). |
| 4, 104 | Set, query, and reset palette colors | Colors 0 to 255, written as `rgb:` or `#` values. |
| 7 | Report the working directory | A `file://` URL. New panes use it (see [Working directory of a new pane](panes-and-tabs.md#working-directory-of-a-new-pane)). |
| 8 | Hyperlinks | The `id=` parameter is supported. |
| 9;9 | Report the working directory on Windows | A Windows path. Other OSC 9 forms are ignored. |
| 10, 11, 12 | Set and query the default foreground, background, and cursor colors | |
| 110, 111, 112 | Reset those three colors | |
| 52 | Copy to the clipboard | Writing only: a program cannot read the clipboard. Data that is not valid base64 clears the clipboard. |

## Modes

| Mode | What it does |
| --- | --- |
| 1 (DECCKM) | Application cursor keys |
| 6 (DECOM) | Origin mode |
| 7 (DECAWM) | Auto-wrap, on by default |
| 12 | Cursor blinking |
| 25 (DECTCEM) | Show the cursor, on by default |
| 47, 1047, 1049 | Alternate screen; 1049 also saves the cursor |
| 1000, 1002, 1003, 1006 | Mouse tracking and the SGR encoding (see [Mouse](#mouse)) |
| 1004 | Focus reporting |
| 1007 | Alternate scroll, on by default |
| 2004 | Bracketed paste |
| 2026 | Synchronized output; a frame is held for at most 150 ms |
| 4 (IRM, set with `CSI 4 h`) | Insert mode |

## Cursor and text attributes

- `CSI Ps SP q` (DECSCUSR) sets the caret shape: 1 and 2 a block, 3 and 4 an
  underline, 5 and 6 a bar, blinking for odd values and steady for even ones.
  0 returns to the [`[cursor]`](configuration.md#cursor) shape and turns
  blinking back on.
- OSC 12 sets the caret color.
- Text can be bold, dim, italic, underlined, reversed, hidden, and struck
  through, in 16 colors, 256 colors, or 24-bit color. Every underline style is
  drawn as a single underline. Underline colors, blinking text, and overlines
  are not drawn.

## Not supported

- Images: sixel, the kitty graphics protocol, and iTerm2 inline images.
- Keyboard protocols: the kitty keyboard protocol and xterm's
  modifyOtherKeys.
- The XTVERSION, DECRQSS, and XTGETTCAP queries.
- Left and right margins (DECLRMM) and reverse video (DECSCNM).
