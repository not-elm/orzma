//! Server-side terminal management: PTY spawn/IO and VT emulation.

pub mod error;
pub mod event;
pub mod pty;
pub mod service;
pub mod vt;

pub use error::{PtyErrorBridge, TerminalError, TerminalResult};
pub use event::TerminalEvent;
pub use service::TerminalService;
pub use service::types::{FrameSubscription, SpawnOptions, TerminalGeometry};

#[cfg(any(test, feature = "test-helpers"))]
pub use service::test_helpers::DamageSnapshot;
