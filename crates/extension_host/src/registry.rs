//! Registry of extension-published webview views addressable by a stable id.
//!
//! The registry is the trust anchor for OSC-driven webviews: only the control
//! plane (`handle_register_view`) writes it, keyed by the authenticated calling
//! extension, so PTY bytes can reference a view by id but never supply content.
use bevy::prelude::*;
use std::collections::HashMap;

/// A view an extension has published for OSC mounting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredView {
    /// HTML entry path (relative to the owning extension dir) the webview loads.
    pub entry: String,
    /// The authenticated extension that published (and serves) this view.
    pub owning_ext: String,
    /// Whether the mounted webview accepts pointer/keyboard input.
    pub interactive: bool,
    /// Host-API namespaces a webview mounting this view may call (namespace
    /// granularity). Empty for control-plane registrations (legacy path).
    pub capabilities: Vec<String>,
}

/// Maps a PTY-facing `view_id` to its trusted, control-plane-registered source.
#[derive(Resource, Default, Debug)]
pub struct ViewRegistry(HashMap<String, RegisteredView>);

impl ViewRegistry {
    /// Inserts or replaces a view registration. Control-plane only.
    pub fn register(&mut self, view_id: String, view: RegisteredView) {
        self.0.insert(view_id, view);
    }

    /// Resolves a `view_id` to its registered view, if any.
    pub fn get(&self, view_id: &str) -> Option<&RegisteredView> {
        self.0.get(view_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_get_roundtrips() {
        let mut reg = ViewRegistry::default();
        reg.register(
            "dashboard".into(),
            RegisteredView {
                entry: "dash.html".into(),
                owning_ext: "memo".into(),
                interactive: true,
                capabilities: vec!["fs".into()],
            },
        );
        assert_eq!(reg.get("dashboard").map(|v| v.interactive), Some(true));
        assert_eq!(
            reg.get("dashboard").map(|v| v.capabilities.clone()),
            Some(vec!["fs".to_string()])
        );
        assert!(reg.get("missing").is_none());
    }
}
