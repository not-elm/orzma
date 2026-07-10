//! One-time migration of a legacy `~/.config/orzma/config.toml` into the new
//! `bevy::settings` location, guarded by a `<prefs>/orzma/.migrated` marker
//! file so it runs at most once per install. Runs synchronously inside
//! `OrzmaConfigsPlugin::build`, between `add_plugins(SettingsPlugin)` and
//! `resolve_and_insert` (`src/configs.rs`), so a freshly-migrated config is
//! reflected in the very first resolve. Persisting the migrated groups to
//! disk happens afterward, in a one-shot `Startup` system, since
//! `bevy::settings`'s save commands are not flushed during `Plugin::build`.

use crate::configs::groups::{
    FontSettings, InactivePaneSettings, KeyboardSettings, MouseSettings, OrzmaSettings,
    ScrollbackSettings, ShortcutSettings, ViModeSettings,
};
use bevy::ecs::schedule::Schedules;
use bevy::prelude::*;
use bevy::settings::SaveSettingsSync;
use orzma_configs::RawSettings;
use orzma_configs::path::{SystemEnv, resolve_config_path};
use std::path::PathBuf;

/// Runs the legacy-config migration once, if needed.
///
/// If a `<prefs>/orzma/.migrated` marker is already present, this is a no-op
/// (already migrated). Otherwise, if a legacy `~/.config/orzma/config.toml`
/// (or `$ORZMA_CONFIG` / `$XDG_CONFIG_HOME` override, via the same
/// precedence `OrzmaConfigs::load` used) exists and is readable, its
/// contents are converted at the presence level via
/// [`RawSettings::from_legacy_toml`] and written into the `bevy::settings`
/// group `Resource`s in `world` — so the very next `resolve_and_insert`
/// call reflects the migrated values. A one-shot `Startup` system is then
/// registered to flush those groups to disk and write the marker.
///
/// Never fatal: an unresolvable preferences directory, an unreadable legacy
/// file (other than simply not existing), or an unwritable marker are all
/// logged via `tracing::warn!` and otherwise ignored — the app always
/// starts, falling back to defaults for whatever could not be migrated.
///
/// A legacy file that exists but is not valid TOML is also skipped — NOT
/// treated as "no legacy config": neither the groups nor the marker are
/// written, so the user's real (currently unparseable) config is left
/// untouched on disk and migration retries on the next launch once they fix
/// it, instead of being silently and permanently replaced with defaults.
pub(super) fn migrate_if_needed(world: &mut World) {
    let Some(prefs_dir) = bevy_platform::dirs::preferences_dir() else {
        tracing::warn!(
            "could not resolve the platform preferences directory; skipping legacy config migration"
        );
        return;
    };
    let settings_dir = prefs_dir.join("orzma");
    let marker_path = settings_dir.join(MARKER_FILE_NAME);
    if marker_path.exists() {
        return;
    }

    let legacy_path = match resolve_config_path(&SystemEnv) {
        Ok(path) => path,
        Err(source) => {
            tracing::warn!(
                %source,
                "could not resolve the legacy orzma config path; skipping migration"
            );
            return;
        }
    };
    let text = match std::fs::read_to_string(&legacy_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(source) => {
            tracing::warn!(
                path = %legacy_path.display(),
                %source,
                "failed to read legacy orzma config; skipping migration"
            );
            return;
        }
    };

    let raw = match RawSettings::from_legacy_toml(&text) {
        Ok(raw) => raw,
        Err(source) => {
            tracing::warn!(
                path = %legacy_path.display(),
                %source,
                "legacy orzma config is not valid TOML; skipping migration until it is fixed and orzma is relaunched"
            );
            return;
        }
    };
    apply_migrated_groups(world, &raw);
    tracing::info!(
        from = %legacy_path.display(),
        to = %settings_dir.join("settings.toml").display(),
        "migrated legacy orzma config to the new settings location"
    );

    world.insert_resource(MigrationMarker(marker_path));
    world
        .resource_mut::<Schedules>()
        .add_systems(Startup, save_and_mark_migrated);
}

/// File name of the migration marker, a sibling of `settings.toml` under the
/// app's `bevy::settings` preferences directory.
const MARKER_FILE_NAME: &str = ".migrated";

/// Resource carrying the migration marker path, consumed by the one-shot
/// `Startup` system [`save_and_mark_migrated`] registered from
/// [`migrate_if_needed`].
#[derive(Resource)]
struct MigrationMarker(PathBuf);

/// Writes each migrated `Raw*` section into its corresponding
/// `bevy::settings` group `Resource`, overwriting whatever default
/// `SettingsPlugin` loaded — there is nothing on disk to overwrite yet,
/// since this only runs before the very first save.
fn apply_migrated_groups(world: &mut World, raw: &RawSettings) {
    world.insert_resource(ShortcutSettings::from(&raw.shortcuts));
    world.insert_resource(ViModeSettings::from(&raw.vi_mode));
    world.insert_resource(FontSettings::from(&raw.font));
    world.insert_resource(MouseSettings::from(&raw.mouse));
    world.insert_resource(KeyboardSettings::from(&raw.keyboard));
    world.insert_resource(InactivePaneSettings::from(&raw.inactive_pane));
    world.insert_resource(OrzmaSettings::from(&raw.orzma));
    world.insert_resource(ScrollbackSettings::from(&raw.scrollback));
}

/// One-shot `Startup` system: synchronously flushes the migrated settings
/// groups to disk, then writes the migration marker.
///
/// # Invariants
///
/// The marker is written only after `SaveSettingsSync::Always` has actually
/// applied (not merely queued) — `world.flush()` drives the queued command
/// to completion before the marker write runs. If the marker were written
/// first, or the save were left queued, a crash between the two would leave
/// a marker on disk with no migrated settings file behind it, permanently
/// losing the user's legacy config.
fn save_and_mark_migrated(world: &mut World) {
    // NOTE: `Always`, not `IfChanged`. `apply_migrated_groups`'s
    // `insert_resource` calls happen inside `OrzmaConfigsPlugin::build`,
    // before the app's first system ever runs, so they land at the exact
    // same `World` change tick `SettingsPlugin::build` captured as its
    // `last_save` baseline. `bevy_settings`' `IfChanged` treats "no tick
    // strictly newer than the baseline" as "nothing changed" and silently
    // skips the write — verified by reverting this to `IfChanged` and
    // observing the migrated settings file never appear on disk even
    // though the groups were correctly populated. `Always` sidesteps that
    // tick-equality edge case entirely.
    world.commands().queue(SaveSettingsSync::Always);
    world.flush();
    let Some(marker) = world.remove_resource::<MigrationMarker>() else {
        return;
    };
    if let Err(source) = std::fs::write(&marker.0, "") {
        tracing::warn!(
            path = %marker.0.display(),
            %source,
            "failed to write the migration marker; migration will retry on next launch"
        );
    }
}
