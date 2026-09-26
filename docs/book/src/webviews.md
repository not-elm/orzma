# Webviews

orzma lets a program running in a pane show live web pages inside the terminal.
The program talks to orzma through the webview protocol:

1. It registers the content — a local directory of web files, a single HTML
   document, or a remote URL — over a control socket that orzma provides to
   every pane.
2. It places the page at a rectangle of terminal cells with an escape sequence
   (on Windows, with a message on the same socket).
3. It exchanges messages with the page in both directions: the page calls the
   program through `window.orzma` and gets replies, and the program sends
   events to the page.

## What this gives you

- **Rich content without leaving the terminal.** Rendered Markdown with
  diagrams and math, or a whole website, appears right where the program puts
  it.
- **Terminal apps with a rich view.** One app can combine a keyboard-driven
  terminal interface with a web page. The page is part of the app's screen and
  scrolls with the text around it.
- **Pages that work with their app.** Because messages flow both ways, the page
  and the app act as one: [orzmd](companion-apps.md#orzmd) jumps between
  headings from wherever you are reading and its search line shows how many
  matches the page found, and [orzbrowser](companion-apps.md#orzbrowser)
  drives link hints inside the page from the keyboard.
- **Any language.** The protocol is a local socket plus one escape sequence, so
  any program can use it. SDKs for Rust and for the page side make it short.
- **Private to your user.** Only programs running as your user can connect to
  orzma's socket, and a program's pages disappear when it exits.

To build one, start with [Building Webview Apps](building-webview-apps.md). The
wire format is in the [Protocol Reference](protocol-reference.md).
