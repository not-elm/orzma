pub mod error;
pub mod pty;
pub mod vt;

pub use error::{PtyErrorBridge, TerminalError, TerminalResult};
#[cfg(any(test, feature = "test-helpers"))]
pub use pty::DamageSnapshot;
pub use pty::{FrameSubscription, SpawnOptions, TerminalEvent, TerminalGeometry, TerminalService};
