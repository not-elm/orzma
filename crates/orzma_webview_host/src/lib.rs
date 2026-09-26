//! The server side of orzma's webviews: the control socket's runtime
//! directory, and the plain-data vocabulary the host shares with the GUI.

pub mod boundary;
pub mod error;
pub mod private_dir;
pub mod protocol;
pub mod runtime_root;
pub mod uds;

pub use private_dir::restrict_to_current_user;

/// The host's public types under one import.
pub mod prelude {
    pub use crate::boundary::{
        ForwardChord, HandleId, MountId, MountSpec, Navigation, PageOutcome, WebviewAsset,
        WebviewEvent,
    };
    pub use crate::error::{
        Refusal, RegisterError, RuntimeRootError, WebviewHostError, WebviewHostResult,
    };
    pub use crate::runtime_root::RuntimeRoot;
}
