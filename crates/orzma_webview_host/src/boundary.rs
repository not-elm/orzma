//! The plain-data vocabulary the webview host and the GUI exchange.

use std::path::PathBuf;

/// The content backing one dynamic handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebviewAsset {
    /// Files served under this absolute root directory.
    Dir(PathBuf),
    /// A single inline HTML document served from memory.
    Inline(Vec<u8>),
}
