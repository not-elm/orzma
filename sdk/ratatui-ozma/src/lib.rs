//! ratatui widget + RPC handler for embedding an ozmux inline webview.
//!
//! Run inside an ozmux pane: [`Ozma::connect`] dials `$OZMUX_SOCK`, [`Webview`]
//! registers content (minting a handle), [`WebviewWidget`] renders it as a
//! ratatui widget, and [`Ozma::flush`] emits the mount OSC after each draw.
#![warn(missing_docs)]

mod error;
mod handler;
mod keychord;
mod osc;
mod protocol;
mod session;
mod webview;
mod widget;

pub use error::{OzmaError, OzmaResult, RpcError};
pub use keychord::KeyChord;
pub use session::Ozma;
pub use webview::{Webview, WebviewHandle};
pub use widget::{Blank, WebviewWidget};
