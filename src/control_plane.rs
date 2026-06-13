//! Native ozmux control plane: a local Unix-socket listener that accepts
//! authenticated dynamic webview registrations (Tier 1) from local programs,
//! mints opaque handles into the `DynamicRegistry`, and tears them down on
//! disconnect or surface despawn. Mirrors the Tokio-free thread model of
//! `ozmux_extension_host::rpc_client`.

mod protocol;
