//! Standalone terminal webview layer: CEF render wiring, the `window.ozma`
//! Tier 1 back-channel, OSC mount/unmount of webviews anchored to terminal
//! cells, and the control socket that mints Tier 1 handles. Decoupled from any
//! multiplexer; the host maps its surfaces onto `OzmaTerminal` entities and
//! drives `KeyboardFocused`.
