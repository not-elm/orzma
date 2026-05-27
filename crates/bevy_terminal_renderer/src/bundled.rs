//! Static byte slices of the bundled JetBrains Mono Nerd Font Mono TTFs.
//!
//! Exposing these as `pub` constants lets downstream crates (the Bevy
//! app's `FontBridgePlugin`) reference the same `include_bytes!`-embedded
//! bytes the renderer's `TerminalFonts::default()` uses, instead of
//! re-embedding identical copies. Without this single source of truth,
//! each `include_bytes!` site in a separate crate produces a distinct
//! static slot (the linker cannot dedup across crate boundaries without
//! LTO), and the binary carries ~10 MB × N copies.

/// Regular-weight JetBrains Mono Nerd Font Mono bytes.
pub const REGULAR: &[u8] =
    include_bytes!("../../../assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-Regular.ttf");

/// Bold-weight JetBrains Mono Nerd Font Mono bytes.
pub const BOLD: &[u8] =
    include_bytes!("../../../assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-Bold.ttf");

/// Italic-style JetBrains Mono Nerd Font Mono bytes.
pub const ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-Italic.ttf");

/// Bold-italic JetBrains Mono Nerd Font Mono bytes.
pub const BOLD_ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-BoldItalic.ttf");

/// Regular-weight UDEVGothic35 bytes (CJK fallback).
pub const FALLBACK_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/udevgothic/UDEVGothic35-Regular.ttf");

/// Bold-weight UDEVGothic35 bytes (CJK fallback).
pub const FALLBACK_BOLD: &[u8] =
    include_bytes!("../assets/fonts/udevgothic/UDEVGothic35-Bold.ttf");

/// Italic-style UDEVGothic35 bytes (CJK fallback).
pub const FALLBACK_ITALIC: &[u8] =
    include_bytes!("../assets/fonts/udevgothic/UDEVGothic35-Italic.ttf");

/// Bold-italic UDEVGothic35 bytes (CJK fallback).
pub const FALLBACK_BOLD_ITALIC: &[u8] =
    include_bytes!("../assets/fonts/udevgothic/UDEVGothic35-BoldItalic.ttf");
