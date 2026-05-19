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

// NOTE: gated strictly on the feature (not `cfg(test)`) because this module
// pulls in `toml` and `sha2` which are themselves optional and only enabled
// by `feature = "test-helpers"`. Built-in unit tests across the crate still
// run via `cargo test -p ozmux_terminal` without needing the feature.
#[cfg(feature = "test-helpers")]
pub mod testing;
