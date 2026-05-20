//! CEF handler implementations.
//!
//! Each handler uses a cef-rs `wrap_*_handler!` macro to bridge the C
//! ref-counted vtable to Rust `Impl*` traits.

pub mod client;
pub mod context_menu;
pub mod display;
pub mod lifespan;
pub mod load;
pub mod render;
pub mod render_process;
pub mod request;
