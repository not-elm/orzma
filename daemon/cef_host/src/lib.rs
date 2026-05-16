//! Library entry for the ozmux cef_host crate. Exposes internal modules so
//! tests can exercise them. The actual binary (`bin/cef_host`) and helper
//! (`bin/cef_helper`) use these via the crate path.

pub mod control;
pub mod handlers;
pub mod input;
pub mod pool;
pub mod post_command;
pub mod shm_writer;
