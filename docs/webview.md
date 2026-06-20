# Webviews

ozmux can render a registered webview **inline in the terminal text
flow** — like the Kitty image protocol, but the surface is a live CEF webview.
The view is composited inside the terminal's own fragment shader, so it scrolls
with the surrounding text, clips at the viewport edges, and sits under the
glyphs (text drawn over it). This is macOS-only today (the headless GPU path
rides the IOSurface accelerated-paint pipeline).

## Enabling it

- Webviews ride the OSC 5379 gate: `osc_webview.enabled` in
  `~/.config/ozmux/config.toml` (default **on**).
- The mounted `<handle>` must first be registered over the control plane (see
  [`dyn-webview.md`](dyn-webview.md)); a program registers content, receives an
  opaque handle, then mounts it with the OSC sequence below.

## Protocol — OSC 5379

A program running in the terminal mounts a view by writing a private OSC 5379
sequence to stdout. The 7-bit `ESC ]` introducer and `ESC \` (ST) terminator
are the canonical forms.

```
mount:    ESC ] 5379 ; mount ; <view_id> ; <rows> ; <cols> [ ; <instance_id> ] ESC \
unmount:  ESC ] 5379 ; unmount [ ; <view_id> [ ; <instance_id> ] ] ESC \
```

- `<view_id>` — an opaque handle minted by the control-plane registration that
  owns the writing surface; must match `^[A-Za-z0-9._-]{1,128}$`.
- `<rows>` / `<cols>` — the rect size in **terminal cells**; integers
  `1..=200` and `1..=400`. The CEF page is laid out at exactly that cell
  rectangle (× DPR); content is not scaled, so the page reflows to fit (no
  crop — a small browser window, not a shrunken screenshot).
- `<instance_id>` — optional, client-assigned, same charset as `<view_id>`.
  It lets the **same `<view_id>` mount more than once** on one terminal:
  `(view_id, instance_id)` is the address. Omitting it selects the implicit
  default instance (the original "one per view_id" behavior is unchanged).
- Out-of-range, non-numeric, or malformed sequences are silently dropped by the
  VT layer.
- `unmount` scopes by how many fields are present: `; <view_id> ;
  <instance_id>` removes that one instance; `; <view_id>` removes *every*
  instance of that view; **no third parameter** (and no trailing `;`) removes
  *every* webview on the terminal. A present-but-empty field — a
  trailing `;`, or `; ; <instance_id>` with no view id — is malformed and
  dropped.

### Anchor and vertical space

The webview is anchored at the **cursor position when the OSC appears in the
stream**. Position the cursor first (e.g. print a heading) and the view mounts
there. Reserving the vertical space is the caller's job: print `<rows>`
newlines after the mount sequence so following output lands below the view.

```sh
# raw printf: heading, then mount memo.main as a 12×48 cell rect + 12 newlines
printf 'memo:\n\033]5379;mount;memo.main;12;48\033\\'
printf '\n%.0s' $(seq 12)
```

Caveats (the rect is anchored to an absolute scrollback line, not reflowed):

- Do not mount while a **scroll region** (DECSTBM) is set or on the
  **alternate screen** — the anchor semantics don't hold there (alt-screen
  mounts are rejected).
- There is no automatic text reflow on width resize; the anchor stays on its
  absolute line and the layout is manual.
- `clear` (CSI 3 J) and scrollback saturation unmount all webviews on
  the terminal. While the scrollback stays saturated, new `mount`
  sequences are rejected until the history drops below the limit (e.g. after
  `clear`).

## Emitting the sequences

Write OSC 5379 directly to stdout from any language. Example in shell:

```sh
# anchor heading, then mount memo.main as a 12×48 cell rect + 12 newlines
printf 'memo:\n'
printf '\033]5379;mount;memo.main;12;48\033\\'
printf '\n%.0s' $(seq 12)

# a second instance of the same view (instanceId = 'b'):
printf '\nmemo (second):\n'
printf '\033]5379;mount;memo.main;12;48;b\033\\'
printf '\n%.0s' $(seq 12)

# later — unmount instance 'b' only:
printf '\033]5379;unmount;memo.main;b\033\\'
# unmount every instance of memo.main:
printf '\033]5379;unmount;memo.main\033\\'
# unmount every webview on the terminal:
printf '\033]5379;unmount\033\\'
```

The `@ozma/web` npm package (`sdk/ozma-web`) is the page-side TypeScript client
for the `window.ozma` bridge inside webview pages — it does not emit OSC sequences.

For a runnable end-to-end client (register over the control plane → mount →
`window.ozma` back-channel) see `examples/dyn_webview_client.rs`:
`cargo run --example dyn_webview_client` inside an ozmux terminal.

## Focus and input

A webview starts render-only. Interaction follows a click-to-focus
model (this matches the `interactive` flag in the control-plane registration;
a non-interactive view never takes focus or input):

- **Click** inside the rect focuses the page (a focus ring appears); the click
  is delivered to the page, not the terminal.
- While focused, **keystrokes and IME** route to the page. The **wheel**
  scrolls the page only while the page is focused *and* the pointer is over it;
  otherwise it scrolls the terminal. **Mouse movement** is forwarded whenever
  the pointer is over the rect (focus or not) — though before the page is first
  focused, hover events may not reach it until CEF establishes its focused
  frame.
- **`Ctrl+Shift+Escape`** (the configurable `release_webview_focus` shortcut)
  returns focus to the terminal; clicking on terminal text outside the rect
  also releases focus.

Two intentional behaviors worth knowing:

- ozmux's own shortcuts still fire while a page is focused, and because key
  events are broadcast, the focused page **also** receives those keystrokes.
- A plain `Escape` typed into a focused page does **not** snap the terminal
  viewport to the bottom (the scroll-to-bottom shortcut is suppressed while an
  webview of the active surface holds focus).

## Limits

- Up to **4** webviews per terminal surface.
- macOS only (the headless GPU compositing path).
- One terminal pane composites its webviews in its own shader pass; the
  text always draws on top of the webview.
