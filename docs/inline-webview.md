# Inline webviews

ozmux can render a registered extension view **inline in the terminal text
flow** — like the Kitty image protocol, but the surface is a live CEF webview.
The view is composited inside the terminal's own fragment shader, so it scrolls
with the surrounding text, clips at the viewport edges, and sits under the
glyphs (text drawn over it). This is macOS-only today (the headless GPU path
rides the IOSurface accelerated-paint pipeline).

## Enabling it

- Inline webviews ride the same OSC 5379 gate as tab-style webviews:
  `osc_webview.enabled` in `~/.config/ozmux/config.toml` (default **on**).
- The bundled `extensions/` directory is **dev-only** — it is discovered only
  under the `debug` cargo feature. Run with `cargo run --features debug` to use
  the bundled `memo` sample; a shipped binary discovers only user-installed
  extensions.

## Protocol — OSC 5379

A program running in the terminal mounts a view by writing a private OSC 5379
sequence to stdout. The 7-bit `ESC ]` introducer and `ESC \` (ST) terminator
are the canonical forms.

```
mount-inline:    ESC ] 5379 ; mount-inline ; <view_id> ; <rows> ; <cols> [ ; <instance_id> ] ESC \
unmount-inline:  ESC ] 5379 ; unmount-inline [ ; <view_id> [ ; <instance_id> ] ] ESC \
```

- `<view_id>` — a view declared in an extension's `ozmux.toml`; must match
  `^[A-Za-z0-9._-]{1,128}$`.
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
- `unmount-inline` scopes by how many fields are present: `; <view_id> ;
  <instance_id>` removes that one instance; `; <view_id>` removes *every*
  instance of that view; **no third parameter** (and no trailing `;`) removes
  *every* inline webview on the terminal. A present-but-empty field — a
  trailing `;`, or `; ; <instance_id>` with no view id — is malformed and
  dropped.

### Anchor and vertical space

The webview is anchored at the **cursor position when the OSC appears in the
stream**. Position the cursor first (e.g. print a heading) and the view mounts
there. Reserving the vertical space is the caller's job: print `<rows>`
newlines after the mount sequence so following output lands below the view.

```sh
# raw printf: heading, then mount memo.main as a 12×48 cell rect + 12 newlines
printf 'memo:\n\033]5379;mount-inline;memo.main;12;48\033\\'
printf '\n%.0s' $(seq 12)
```

Caveats (the rect is anchored to an absolute scrollback line, not reflowed):

- Do not mount while a **scroll region** (DECSTBM) is set or on the
  **alternate screen** — the anchor semantics don't hold there (alt-screen
  mounts are rejected).
- There is no automatic text reflow on width resize; the anchor stays on its
  absolute line and the layout is manual.
- `clear` (CSI 3 J) and scrollback saturation unmount all inline webviews on
  the terminal. While the scrollback stays saturated, new `mount-inline`
  sequences are rejected until the history drops below the limit (e.g. after
  `clear`).

## SDK helper

`@ozmux/sdk/inline` builds the sequences (with validation and the newline
reservation) so extension tooling doesn't hand-roll escape codes:

```ts
import { mountInline, unmountInline } from '@ozmux/sdk/inline';

process.stdout.write('memo:\n');                          // anchor heading
process.stdout.write(mountInline('memo.main', { rows: 12, cols: 48 }));
// a second instance of the same view, addressed by instanceId:
process.stdout.write('\nmemo (second):\n');
process.stdout.write(mountInline('memo.main', { rows: 12, cols: 48, instanceId: 'b' }));
// later:
process.stdout.write(unmountInline('memo.main', 'b'));    // just instance 'b'
process.stdout.write(unmountInline('memo.main'));         // every instance of memo.main
process.stdout.write(unmountInline());                    // every inline webview
```

`mountInline` returns the OSC sequence followed by `rows` newlines as one
string (one atomic `write`). It throws `RangeError` on an invalid view id or
out-of-range geometry rather than emitting a sequence the terminal would drop.

The bundled sample is `extensions/memo/mount.ts` — run it inside an ozmux
terminal (`cargo run --features debug`) with `node extensions/memo/mount.ts`
(or `pnpm --filter memo mount`).

## Focus and input

An inline webview starts render-only. Interaction follows a click-to-focus
model (this matches the `interactive = true` flag in the view's `ozmux.toml`;
a non-interactive view never takes focus or input):

- **Click** inside the rect focuses the page (a focus ring appears); the click
  is delivered to the page, not the terminal.
- While focused, **keystrokes and IME** route to the page. The **wheel**
  scrolls the page only while the page is focused *and* the pointer is over it;
  otherwise it scrolls the terminal. **Mouse movement** is forwarded whenever
  the pointer is over the rect (focus or not) — though before the page is first
  focused, hover events may not reach it until CEF establishes its focused
  frame.
- **`Ctrl+Shift+Escape`** (the configurable `release_inline_focus` shortcut)
  returns focus to the terminal; clicking on terminal text outside the rect
  also releases focus.

Two intentional behaviors worth knowing:

- ozmux's own shortcuts still fire while a page is focused, and because key
  events are broadcast, the focused page **also** receives those keystrokes.
- A plain `Escape` typed into a focused page does **not** snap the terminal
  viewport to the bottom (the scroll-to-bottom shortcut is suppressed while an
  inline webview of the active surface holds focus).

## Limits

- Up to **4** inline webviews per terminal surface.
- macOS only (the headless GPU compositing path).
- One terminal pane composites its inline webviews in its own shader pass; the
  text always draws on top of the webview.
