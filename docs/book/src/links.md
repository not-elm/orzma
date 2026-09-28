# Links

orzma opens two kinds of links with the mouse: hyperlinks that a program
marks up with OSC 8, and URLs it finds in plain text.

## Opening a link

Hold `Cmd` on macOS or `Ctrl` on Windows and Linux, point at a link, and
left-click it. orzma hands the URL to your system's default handler, such as
your web browser or mail client.

While the modifier is held over a link, the pointer turns into a hand and the
link is underlined in an accent color.

A modified click on a link opens it even while a program such as vim or tmux
tracks the mouse, and that program does not see the click. Anywhere else, the
click reaches the program as usual.

## OSC 8 hyperlinks

Programs such as `ls --hyperlink` mark text as a link with the OSC 8 escape
sequence. These links are always underlined, and the underline takes the
accent color while you hold the modifier over one.

## URLs in plain text

orzma also recognizes URLs in ordinary output, such as the address a
development server prints. A detected URL is not underlined until you hold
the modifier and point at it.

- URLs that start with `http://`, `https://`, `ftp://`, or `mailto:` are
  detected. Addresses without a scheme, such as `example.com`, and file paths
  are not.
- A URL that wraps onto the next row is detected as one URL.
- A sentence's final `.` or `,`, and a closing `)` or `]` without an opening
  partner inside the URL, are not part of it.
- A URL ends at a wide character, such as Japanese text written right after
  it (`https://example.com/docsを参照`), and at a closing quote or dash. Any
  other non-ASCII character right after a URL, such as the `…` of a truncated
  table cell, means the URL is not linked at all. An address shortened with
  three ASCII dots (`...`) is the exception: the dots are trimmed like a
  sentence's final `.`, so the shortened address is linked as it stands.
- A URL that runs past the top or bottom edge of the window is not linked
  until it is fully in view.
- A URL that a program splits across rows with its own line breaks is not
  joined.
- When the text of an OSC 8 hyperlink also looks like a URL, the OSC 8 target
  is the one that opens.

## Allowed schemes

orzma opens only `http`, `https`, `mailto`, and `ftp` URLs, whether they come
from OSC 8 or from plain text.
