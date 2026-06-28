# Ozma Webview Protocol

> ozmux is in early development; this wire format is documented as it is today
> and may change between releases. The [SDKs](#sdks) track these changes for
> you — prefer them unless you are implementing your own client.

The Ozma Webview protocol lets a local program running inside an ozmux pane
render webview content inline in the terminal and exchange messages with the
page. It spans three surfaces:

1. **The control socket** — a local Unix-socket connection over which a program
   registers content, manages it, and routes the page back-channel.
2. **OSC 5379** — terminal escape sequences that mount and unmount registered
   content at a cell rectangle.
3. **The `window.ozma` bridge** — an in-page JavaScript API the webview uses to
   call, subscribe to, and emit events to the registering program.

Three actors participate: the **registering program** (running in a pane), the
**ozmux host**, and the **webview page**. A registration is a *Tier 1* (dynamic,
runtime-registered) webview — the only kind this protocol describes.

End to end: a program connects to the control socket, registers content and
receives an opaque **handle**, writes an `OSC 5379;mount;<handle>;…` sequence to
display it, and then talks to the page through the `window.ozma` bridge routed
over the same control socket. Unmounting (or disconnecting) tears it down.

## Architecture at a glance

```text
 registering program              ozmux host                  webview page
 (inside an ozmux pane)
        │  reads $OZMA_SOCK / $OZMA_TOKEN from its env
        │  hello{token} ───────────────►│
        │  register{kind,…} ───────────►│
        │◄─────────────── {ok,handle} ──│
        │  OSC 5379;mount;handle;r;c ──►│  mount ozma://handle/ ───►│ load page
        │                               │◄──── window.ozma.call ────│
        │◄──── {op:call,reqId,method} ──│                           │
        │  {op:reply,reqId,value} ─────►│──── resolve Promise ─────►│
        │  {op:emit,event,payload} ────►│──── window.ozma.on ──────►│
        │◄──── {op:event,…} ◄ window.ozma.emit ─────────────────────│
        │  OSC 5379;unmount;handle ────►│  despawn webview ─────────►│
```

The control socket carries every horizontal arrow between the program and the
host; OSC 5379 carries the mount/unmount; the page bridge carries the
`window.ozma` arrows on the right.
