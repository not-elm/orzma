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
use std::path::{Path, PathBuf};

/// Runs the legacy-config migration once, if needed.
///
/// If a `<prefs>/orzma/.migrated` marker is already present, this is a no-op
/// (already migrated). Otherwise, if a legacy `~/.config/orzma/config.toml`
/// (or `$ORZMA_CONFIG` / `$XDG_CONFIG_HOME` override, via the same
/// precedence `orzma_configs::path::resolve_config_path` applies) exists and
/// is readable, its contents are converted at the presence level via
/// [`RawSettings::from_legacy_toml`] and written into the `bevy::settings`
/// group `Resource`s in `world` — so the very next `resolve_and_insert`
/// call reflects the migrated values. A one-shot `Startup` system is then
/// registered to flush those groups to disk and write the marker.
///
/// When there is NO legacy config to migrate (the legacy path does not
/// exist), the marker is still written immediately — see
/// [`mark_migrated_without_legacy_config`] — so a legacy config file that
/// appears later (dotfiles sync, an old build reinstalled) can never
/// silently clobber `settings.toml` the user has since edited through the
/// new UI.
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
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            mark_migrated_without_legacy_config(&settings_dir);
            return;
        }
        Err(source) => {
            tracing::warn!(
                path = %legacy_path.display(),
                %source,
                "failed to read legacy orzma config; skipping migration"
            );
            return;
        }
    };

    let (raw, diags) = match RawSettings::from_legacy_toml(&text) {
        Ok(parsed) => parsed,
        Err(source) => {
            tracing::warn!(
                path = %legacy_path.display(),
                %source,
                "legacy orzma config is not valid TOML; skipping migration until it is fixed and orzma is relaunched"
            );
            return;
        }
    };
    for d in &diags {
        tracing::warn!(target: "orzma::config::migrate", "{}", d.message);
    }
    apply_migrated_groups(world, &raw);
    tracing::info!(
        from = %legacy_path.display(),
        to = %settings_dir.join(SETTINGS_FILE_NAME).display(),
        "migrated legacy orzma config to the new settings location"
    );

    world.insert_resource(MigrationMarker(marker_path));
    world
        .resource_mut::<Schedules>()
        .add_systems(Startup, save_and_mark_migrated);
}

/// Marks migration as done with NOTHING to migrate: creates `settings_dir`
/// if needed and writes an empty `.migrated` marker inside it directly
/// (no `Startup` system, no save to wait for — there are no groups to
/// flush). Called from `migrate_if_needed`'s "no legacy config" path so a
/// legacy config file that appears later cannot be mistaken for "not yet
/// migrated" and clobber a `settings.toml` the user may have since written
/// through the new UI. Never fatal: an I/O failure just means migration
/// retries on the next launch, exactly like the pre-existing no-op-on-error
/// paths elsewhere in this module.
fn mark_migrated_without_legacy_config(settings_dir: &Path) {
    if let Err(source) = std::fs::create_dir_all(settings_dir) {
        tracing::warn!(
            path = %settings_dir.display(),
            %source,
            "failed to create the settings directory for the migration marker; migration will retry on next launch"
        );
        return;
    }
    let marker_path = settings_dir.join(MARKER_FILE_NAME);
    if let Err(source) = std::fs::write(&marker_path, "") {
        tracing::warn!(
            path = %marker_path.display(),
            %source,
            "failed to write the migration marker; migration will retry on next launch"
        );
    }
}

/// File name of the migration marker, a sibling of `settings.toml` under the
/// app's `bevy::settings` preferences directory.
const MARKER_FILE_NAME: &str = ".migrated";

/// File name `bevy::settings` writes the persisted settings under, a sibling
/// of the migration marker.
const SETTINGS_FILE_NAME: &str = "settings.toml";

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
/// groups to disk, then writes the migration marker — but ONLY once the
/// settings file the flush was supposed to produce is actually confirmed
/// present on disk (see [`migrated_settings_exists`]).
///
/// # Invariants
///
/// `bevy::settings`'s save is best-effort: a write failure is logged and
/// swallowed inside the crate, with no `Result` or error signal reaching
/// this caller. Without the existence check below, a failed save would
/// still be followed by an unconditional marker write, so the very next
/// launch would see `.migrated` present, skip migration entirely, and the
/// user's legacy config would be gone for good with nothing on disk to
/// show for it. The existence check makes that failure observable and
/// keeps migration armed to retry instead.
///
/// The marker is written only after `SaveSettingsSync::Always` has actually
/// applied (not merely queued) — `world.flush()` drives the queued command
/// to completion before the marker write runs. If the marker were written
/// first, or the save were left queued, a crash between the two would leave
/// a marker on disk with no migrated settings file behind it, permanently
/// losing the user's legacy config.
///
/// The marker's parent directory is created explicitly rather than assumed
/// to already exist from the `SaveSettingsSync::Always` write above: if a
/// future change ever made that save target a different path (or fail
/// silently), a missing settings directory here would make the marker write
/// fail every launch, and the app would re-run migration forever.
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
    if !migrated_settings_exists(&marker.0) {
        tracing::warn!(
            path = %marker.0.display(),
            "the migrated settings file was not found on disk after the save; the save likely \
             failed; migration will retry on next launch"
        );
        return;
    }
    if let Some(dir) = marker.0.parent()
        && let Err(source) = std::fs::create_dir_all(dir)
    {
        tracing::warn!(
            path = %dir.display(),
            %source,
            "failed to create the settings directory for the migration marker; migration will retry on next launch"
        );
        return;
    }
    if let Err(source) = std::fs::write(&marker.0, "") {
        tracing::warn!(
            path = %marker.0.display(),
            %source,
            "failed to write the migration marker; migration will retry on next launch"
        );
    }
}

/// True when `<marker_path's parent>/settings.toml` exists on disk — i.e.
/// the `SaveSettingsSync::Always` write in [`save_and_mark_migrated`]
/// actually landed. `bevy_settings`'s save is best-effort with no failure
/// signal back to the caller, so this existence check is the only way to
/// detect a failed save before committing to the `.migrated` marker.
fn migrated_settings_exists(marker_path: &Path) -> bool {
    marker_path
        .parent()
        .is_some_and(|dir| dir.join(SETTINGS_FILE_NAME).exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn migrated_settings_exists_true_when_settings_toml_present() {
        let tempdir = TempDir::new().expect("create tempdir");
        let settings_dir = tempdir.path().join("orzma");
        std::fs::create_dir_all(&settings_dir).expect("create settings dir");
        std::fs::write(settings_dir.join(SETTINGS_FILE_NAME), "").expect("write settings.toml");
        let marker_path = settings_dir.join(MARKER_FILE_NAME);
        assert!(migrated_settings_exists(&marker_path));
    }

    #[test]
    fn migrated_settings_exists_false_when_settings_toml_missing() {
        let tempdir = TempDir::new().expect("create tempdir");
        let settings_dir = tempdir.path().join("orzma");
        std::fs::create_dir_all(&settings_dir).expect("create settings dir");
        let marker_path = settings_dir.join(MARKER_FILE_NAME);
        assert!(
            !migrated_settings_exists(&marker_path),
            "a failed save must not be mistaken for success"
        );
    }

    #[test]
    fn migrated_settings_exists_false_when_settings_dir_itself_is_missing() {
        let tempdir = TempDir::new().expect("create tempdir");
        let settings_dir = tempdir.path().join("orzma");
        let marker_path = settings_dir.join(MARKER_FILE_NAME);
        assert!(!migrated_settings_exists(&marker_path));
    }

    #[test]
    fn mark_migrated_without_legacy_config_creates_dir_and_marker() {
        let tempdir = TempDir::new().expect("create tempdir");
        let settings_dir = tempdir.path().join("orzma");
        mark_migrated_without_legacy_config(&settings_dir);
        assert!(
            settings_dir.join(MARKER_FILE_NAME).exists(),
            "the marker must be written even with nothing to migrate, so a legacy config \
             appearing later cannot clobber settings.toml"
        );
    }

    #[test]
    fn mark_migrated_without_legacy_config_is_idempotent_on_existing_dir() {
        let tempdir = TempDir::new().expect("create tempdir");
        let settings_dir = tempdir.path().join("orzma");
        std::fs::create_dir_all(&settings_dir).expect("pre-create settings dir");
        mark_migrated_without_legacy_config(&settings_dir);
        assert!(settings_dir.join(MARKER_FILE_NAME).exists());
    }
}
