//! ratatui widget + RPC handler for embedding an orzma webview.
//!
//! Run inside an orzma pane: [`Orzma::connect`] dials `$ORZMA_SOCK`, [`Webview`]
//! registers content (minting a handle), [`WebviewWidget`] renders it as a
//! ratatui widget, and [`OrzmaBackend`] (wrapping the terminal backend) emits the
//! mount OSC during each draw — no separate flush call.
#![warn(missing_docs)]

mod backend;
mod error;
mod events;
mod handler;
mod keychord;
mod osc;
mod protocol;
mod session;
mod webview;
mod widget;

pub use backend::OrzmaBackend;
pub use error::{OrzmaError, OrzmaResult, RpcError};
pub use keychord::KeyChord;
pub use session::{FramePlacements, Orzma};
pub use webview::{Webview, WebviewHandle};
pub use widget::{WebviewDefaultPlaceholder, WebviewWidget};
