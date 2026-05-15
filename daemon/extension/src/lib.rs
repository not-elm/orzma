//! Extension lifecycle: spawns Node extensions and owns the per-PID runtime tree.

pub mod error;
pub mod handle;
pub mod registry;
pub mod runtime;

pub use registry::{ExtensionInfo, ExtensionRegistry};
