//! CEF handler implementations (RenderHandler / ClientHandler / LifeSpanHandler).
//!
//! Each handler uses cef-rs's `wrap_*` macro to bridge the C ref-counted vtable
//! to Rust `Impl*` traits.

pub mod client;
pub mod lifespan;
pub mod render;
