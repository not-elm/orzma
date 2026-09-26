//! The server side of orzma's webviews: the control socket's runtime
//! directory, and the plain-data vocabulary the host shares with the GUI.

pub mod boundary;
pub mod host;
pub mod private_dir;
pub mod uds;

pub use boundary::WebviewAsset;
pub use private_dir::restrict_to_current_user;
