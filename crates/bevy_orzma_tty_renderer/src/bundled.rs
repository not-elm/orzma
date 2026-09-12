//! Static byte slices of the font TTFs `TerminalFonts::default()` loads.
//!
//! A crate that needs the same fonts references these constants.

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
pub const FALLBACK_BOLD: &[u8] = include_bytes!("../assets/fonts/udevgothic/UDEVGothic35-Bold.ttf");

/// Italic-style UDEVGothic35 bytes (CJK fallback).
pub const FALLBACK_ITALIC: &[u8] =
    include_bytes!("../assets/fonts/udevgothic/UDEVGothic35-Italic.ttf");

/// Bold-italic UDEVGothic35 bytes (CJK fallback).
pub const FALLBACK_BOLD_ITALIC: &[u8] =
    include_bytes!("../assets/fonts/udevgothic/UDEVGothic35-BoldItalic.ttf");

/// Noto Sans Symbols 2 bytes (symbol/dingbat fallback).
///
/// Covers the Miscellaneous Symbols, Dingbats, and Geometric Shapes
/// blocks (e.g. ☐ ☑ ☒ ✔) that neither the primary nor the CJK fallback
/// carries. This single regular face serves all faces.
pub const SYMBOL_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/notosanssymbols2/NotoSansSymbols2-Regular.ttf");
