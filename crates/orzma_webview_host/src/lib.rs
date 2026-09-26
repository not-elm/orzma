//! The server side of orzma's webviews: the control socket's runtime
//! directory, and the plain-data vocabulary the host shares with the GUI.

pub mod boundary;
pub mod control_socket;
pub mod error;
pub mod host;
pub mod private_dir;
pub mod protocol;
pub mod runtime_root;
pub mod uds;

pub use private_dir::restrict_to_current_user;

/// The host's public types under one import.
pub mod prelude {
    pub use crate::boundary::{
        ForwardChord, HandleId, MountId, MountSpec, Navigation, PageOutcome, WebviewAsset,
        WebviewCommand, WebviewEvent,
    };
    pub use crate::control_socket::{ConnectionId, ControlEvent, ControlSocket};
    pub use crate::error::{
        Refusal, RegisterError, RuntimeRootError, WebviewHostError, WebviewHostResult,
    };
    pub use crate::host::{
        HostOutput, MuxRequest, PaneKey, PlacementSignal, ValidatedRegistration, WebviewHost,
    };
    pub use crate::runtime_root::RuntimeRoot;
}
