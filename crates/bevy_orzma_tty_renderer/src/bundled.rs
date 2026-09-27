//! Static byte slices of the font TTFs `TerminalFonts::default()` loads.
//!
//! Each font's bytes are embedded once in the final binary, whichever crate
//! reads them.

/// Regular-weight JetBrains Mono Nerd Font Mono bytes.
pub static REGULAR: &[u8] =
    include_bytes!("../../../assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-Regular.ttf");

/// Bold-weight JetBrains Mono Nerd Font Mono bytes.
pub static BOLD: &[u8] =
    include_bytes!("../../../assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-Bold.ttf");

/// Italic-style JetBrains Mono Nerd Font Mono bytes.
pub static ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-Italic.ttf");

/// Bold-italic JetBrains Mono Nerd Font Mono bytes.
pub static BOLD_ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-BoldItalic.ttf");

/// Regular-weight UDEVGothic35 bytes (CJK fallback).
pub static FALLBACK_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/udevgothic/UDEVGothic35-Regular.ttf");

/// Bold-weight UDEVGothic35 bytes (CJK fallback).
pub static FALLBACK_BOLD: &[u8] =
    include_bytes!("../assets/fonts/udevgothic/UDEVGothic35-Bold.ttf");

/// Italic-style UDEVGothic35 bytes (CJK fallback).
pub static FALLBACK_ITALIC: &[u8] =
    include_bytes!("../assets/fonts/udevgothic/UDEVGothic35-Italic.ttf");

/// Bold-italic UDEVGothic35 bytes (CJK fallback).
pub static FALLBACK_BOLD_ITALIC: &[u8] =
    include_bytes!("../assets/fonts/udevgothic/UDEVGothic35-BoldItalic.ttf");

/// Noto Sans Symbols 2 bytes (symbol/dingbat fallback).
///
/// Covers the Miscellaneous Symbols, Dingbats, and Geometric Shapes
/// blocks (e.g. ☐ ☑ ☒ ✔) that neither the primary nor the CJK fallback
/// carries. This single regular face serves all faces.
pub static SYMBOL_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/notosanssymbols2/NotoSansSymbols2-Regular.ttf");
