//! Resolves the `bevy::settings` `SettingsGroup` resources
//! (`configs::groups`) into `OrzmaConfigs` at `Plugin::build` and exposes
//! the result as a Bevy Resource. Resolution diagnostics (duplicate direct
//! or prefix chords, duplicate `[vi-mode]` keys, a leader that shadows a
//! direct binding, prefix bindings with no leader, an unmappable leader
//! key, an out-of-range font size, an unparseable `[font]` face `style`)
//! are logged via `tracing::warn!` and the offending entries fall back to
//! defaults — nothing here is fatal, so the GUI always starts.

use bevy::prelude::*;
use bevy::settings::SettingsPlugin;
use groups::{
    FontSettings, InactivePaneSettings, KeyboardSettings, MouseSettings, OrzmaSettings,
    ScrollbackSettings, ShortcutSettings, ViModeSettings,
};
use orzma_configs::OrzmaConfigs;

mod groups;

/// Bevy Resource wrapping the resolved `OrzmaConfigs`.
#[derive(Resource, Debug, Default, Deref)]
pub(crate) struct OrzmaConfigsResource(pub(crate) OrzmaConfigs);

/// Bevy Plugin that resolves orzma config from the `bevy::settings` groups
/// at `Plugin::build` and inserts the result as [`OrzmaConfigsResource`].
pub(crate) struct OrzmaConfigsPlugin;

impl Plugin for OrzmaConfigsPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: register_type BEFORE SettingsPlugin (else map/nested fields load empty).
        app.register_type::<ShortcutSettings>()
            .register_type::<ViModeSettings>()
            .register_type::<FontSettings>()
            .register_type::<MouseSettings>()
            .register_type::<KeyboardSettings>()
            .register_type::<InactivePaneSettings>()
            .register_type::<ScrollbackSettings>()
            .register_type::<OrzmaSettings>()
            .add_plugins(SettingsPlugin::new("orzma"));
        resolve_and_insert(app.world_mut());
    }
}

/// Crate-internal mutex guarding `ORZMA_CONFIG` env-var mutations across
/// tests. Any test (in any module) that mutates the process env BEFORE
/// constructing `OrzmaConfigsPlugin` (or anything else that calls
/// `OrzmaConfigs::load`) MUST acquire this guard for the duration
/// of the construction.
#[cfg(test)]
pub(crate) fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::Mutex;
    static ENV_GUARD: Mutex<()> = Mutex::new(());
    ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner())
}

/// Reads the settings groups (or `$ORZMA_CONFIG`, Task 8), resolves them,
/// logs diagnostics, and inserts [`OrzmaConfigsResource`]. Extracted from
/// `build` so tests can exercise it without adding `SettingsPlugin` (which
/// reads the real OS prefs dir).
fn resolve_and_insert(world: &mut World) {
    let raw = groups::collect_raw(world);
    let (cfg, diags) = raw.resolve();
    for d in &diags {
        tracing::warn!(target: "orzma::config", "{}", d.message);
    }
    world.insert_resource(OrzmaConfigsResource(cfg));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_and_insert_produces_default_resource() {
        // NOTE: Hermetic test: no SettingsPlugin, no disk. collect_raw falls back to Default
        // for any group not present, so an empty world resolves to the defaults.
        let mut app = App::new();
        resolve_and_insert(app.world_mut());
        let res = app
            .world()
            .get_resource::<OrzmaConfigsResource>()
            .expect("resource inserted");
        assert_eq!(res.0, OrzmaConfigs::default());
    }
}
