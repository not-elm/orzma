//! Static byte slices of the bundled Iosevka Term Nerd Font Mono TTFs.
//!
//! Exposing these as `pub` constants lets downstream crates (the Bevy
//! app's `FontBridgePlugin`) reference the same `include_bytes!`-embedded
//! bytes the renderer's `TerminalFonts::default()` uses, instead of
//! re-embedding identical copies. Without this single source of truth,
//! each `include_bytes!` site in a separate crate produces a distinct
//! static slot (the linker cannot dedup across crate boundaries without
//! LTO), and the binary carries ~52 MB × N copies.

/// Regular-weight Iosevka Term Nerd Font Mono bytes.
pub const REGULAR: &[u8] =
    include_bytes!("../../../assets/fonts/iosevka/IosevkaTermNerdFontMono-Regular.ttf");

/// Bold-weight Iosevka Term Nerd Font Mono bytes.
pub const BOLD: &[u8] =
    include_bytes!("../../../assets/fonts/iosevka/IosevkaTermNerdFontMono-Bold.ttf");

/// Italic-style Iosevka Term Nerd Font Mono bytes.
pub const ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/iosevka/IosevkaTermNerdFontMono-Italic.ttf");

/// Bold-italic Iosevka Term Nerd Font Mono bytes.
pub const BOLD_ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/iosevka/IosevkaTermNerdFontMono-BoldItalic.ttf");
