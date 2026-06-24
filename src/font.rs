//! Bridge between `OzmuxConfigsResource.font` and the renderer's
//! `TerminalFonts` Resource, plus the `TerminalUiFont` handle that UI
//! text builders consume. Runs at Startup, ordered
//! `.before(TerminalFontInitSet::InitCellMetrics)` so the renderer's
//! cell-metrics computation sees any overridden font.
//!
//! Startup-only: font changes require a process restart. If a future
//! feature adds config hot-reload, `bridge_font_config` must move to a
//! change-detection system in Update (and additionally re-issue cell
//! metrics + invalidate the glyph atlas — see the renderer crate).

use crate::configs::OzmuxConfigsResource;
use bevy::prelude::*;
use bevy::text::{CosmicFontSystem, Font};
use ozma_tty_renderer::{FontFace, TerminalFontInitSet, TerminalFontSize, TerminalFonts, bundled};
use ozmux_configs::font::FontConfig;
use ozmux_configs::path::{SystemEnv, expand_user_path};
use std::path::Path;

/// Strong handle to the UI font asset (regular face). Inserted by
/// `bridge_font_config`. UI builders read this resource to set
/// `TextFont { font, ... }`.
#[derive(Resource, Clone)]
pub struct TerminalUiFont(pub Handle<Font>);

/// Strong handle to the bundled Nerd Font, used only for the window-bar
/// powerline separator glyphs (U+E0B0 / U+E0B2). Independent of
/// `TerminalUiFont` so a user font override (which may lack those glyphs)
/// cannot turn the separators into tofu.
#[derive(Resource, Clone)]
pub struct PowerlineFont(pub Handle<Font>);

/// Bevy plugin that wires `bridge_font_config` into Startup.
pub struct FontBridgePlugin;

impl Plugin for FontBridgePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            (
                bridge_font_config.before(TerminalFontInitSet::InitCellMetrics),
                register_cjk_fallback_with_cosmic,
            ),
        );
    }
}

/// Loads bytes for one face. Returns the file contents if `path` is set
/// AND expansion succeeds AND read succeeds; otherwise returns the
/// bundled bytes. Failures are warned, not propagated — partial override
/// is a feature.
fn load_face_bytes(path: Option<&Path>, bundled: &'static [u8], face: FontFace) -> Vec<u8> {
    let Some(p) = path else {
        return bundled.to_vec();
    };
    let Some(expanded) = expand_user_path(p, &SystemEnv) else {
        tracing::warn!(?face, path = %p.display(), "font path expansion failed; using bundled");
        return bundled.to_vec();
    };
    match std::fs::read(&expanded) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(?face, path = %expanded.display(), %err, "font path read failed; using bundled");
            bundled.to_vec()
        }
    }
}

/// Validates one face's bytes by attempting to parse them as a font.
/// Returns the original bytes on success; on failure (parse error), warns
/// and returns the bundled bytes for that face.
fn validate_or_bundled(bytes: Vec<u8>, bundled: &'static [u8], face: FontFace) -> Vec<u8> {
    // Zero-copy validation: FontRef borrows the slice instead of consuming
    // a Vec, so we don't clone 13 MB of font data just to throw away the
    // parsed FontArc immediately after.
    match ab_glyph::FontRef::try_from_slice(&bytes) {
        Ok(_) => bytes,
        Err(err) => {
            tracing::warn!(
                ?face,
                %err,
                "font face failed to parse; substituting bundled bytes for THAT face only"
            );
            bundled.to_vec()
        }
    }
}

/// Registers the bundled CJK fallback font directly into cosmic-text's
/// fontdb, making it discoverable as a script-fallback for spans whose
/// primary `TextFont` lacks CJK coverage.
///
/// NOTE: Bevy's `Assets<Font>::add(...)` is NOT sufficient here —
/// `bevy_text::load_font_to_fontdb` only runs when a `TextFont`
/// handle pointing at the asset reaches the text pipeline
/// (`bevy_text-0.18.1/src/pipeline.rs:597-620`). For a fallback that
/// is never referenced as a primary `TextFont`, call
/// `db_mut().load_font_source(...)` explicitly so cosmic-text's
/// `FontFallbackIter::other_i` last-resort loop sees it.
fn register_cjk_fallback_with_cosmic(mut font_system: ResMut<CosmicFontSystem>) {
    let source = cosmic_text::fontdb::Source::Binary(std::sync::Arc::new(
        ozma_tty_renderer::bundled::FALLBACK_REGULAR,
    )
        as std::sync::Arc<dyn AsRef<[u8]> + Send + Sync>);
    font_system.db_mut().load_font_source(source);
    tracing::info!(
        target: "ozmux::font",
        "registered UDEVGothic35-Regular into cosmic-text fontdb",
    );
}

fn bridge_font_config(
    mut commands: Commands,
    mut fonts_assets: ResMut<Assets<Font>>,
    mut terminal_fonts: ResMut<TerminalFonts>,
    mut font_size: ResMut<TerminalFontSize>,
    configs: Res<OzmuxConfigsResource>,
) {
    font_size.0 = configs.font.size;
    let font: &FontConfig = &configs.font;

    let powerline = make_ui_font_handle(bundled::REGULAR.to_vec(), &mut fonts_assets);
    commands.insert_resource(PowerlineFont(powerline.clone()));

    // Fast path: no override → use the renderer's default TerminalFonts
    // (already inserted by TerminalFontPlugin) and feed the same bundled
    // regular bytes to Assets<Font>. Skips ~52 MB of needless allocations
    // and ~5 redundant ab_glyph / bevy_text parses on the common cold path.
    let no_override = font.normal.is_none()
        && font.bold.is_none()
        && font.italic.is_none()
        && font.bold_italic.is_none();
    if no_override {
        commands.insert_resource(TerminalUiFont(powerline));
        return;
    }

    let regular_bytes =
        load_face_bytes(font.normal.as_deref(), bundled::REGULAR, FontFace::Regular);
    let bold_bytes = load_face_bytes(font.bold.as_deref(), bundled::BOLD, FontFace::Bold);
    let italic_bytes = load_face_bytes(font.italic.as_deref(), bundled::ITALIC, FontFace::Italic);
    let bold_italic_bytes = load_face_bytes(
        font.bold_italic.as_deref(),
        bundled::BOLD_ITALIC,
        FontFace::BoldItalic,
    );

    // NOTE: validate-per-face is load-bearing. Without it, a corrupt
    // override on ANY face drops ALL overrides — the all-or-nothing
    // regression that commit 09213e7 fixed.
    let regular_bytes = validate_or_bundled(regular_bytes, bundled::REGULAR, FontFace::Regular);
    let bold_bytes = validate_or_bundled(bold_bytes, bundled::BOLD, FontFace::Bold);
    let italic_bytes = validate_or_bundled(italic_bytes, bundled::ITALIC, FontFace::Italic);
    let bold_italic_bytes = validate_or_bundled(
        bold_italic_bytes,
        bundled::BOLD_ITALIC,
        FontFace::BoldItalic,
    );

    let ui_regular_bytes = regular_bytes.clone();

    let new_fonts = TerminalFonts::from_bytes(
        regular_bytes,
        bold_bytes,
        italic_bytes,
        bold_italic_bytes,
        ozma_tty_renderer::bundled::FALLBACK_REGULAR.to_vec(),
        ozma_tty_renderer::bundled::FALLBACK_BOLD.to_vec(),
        ozma_tty_renderer::bundled::FALLBACK_ITALIC.to_vec(),
        ozma_tty_renderer::bundled::FALLBACK_BOLD_ITALIC.to_vec(),
    )
    .expect("validated bytes must parse");
    *terminal_fonts = new_fonts;

    let handle = make_ui_font_handle(ui_regular_bytes, &mut fonts_assets);
    commands.insert_resource(TerminalUiFont(handle));
}

/// Builds the UI-side `Handle<Font>` from the regular face bytes. On
/// `Font::try_from_bytes` rejection (rare; ab_glyph and bevy_text use
/// different parser families, so a font accepted by one may be rejected
/// by the other), falls back to Bevy's default FiraMono via
/// `Handle::<Font>::default()`.
fn make_ui_font_handle(regular_bytes: Vec<u8>, fonts_assets: &mut Assets<Font>) -> Handle<Font> {
    match Font::try_from_bytes(regular_bytes) {
        Ok(font_asset) => fonts_assets.add(font_asset),
        Err(err) => {
            // ERROR not warn: the user's terminal and UI overlay will now
            // render in different typefaces (terminal: user-supplied or
            // bundled JetBrains Mono; UI: Bevy's FiraMono), a visible
            // inconsistency that operators should know about.
            tracing::error!(
                %err,
                "bevy_text::Font::try_from_bytes rejected the regular face; \
                 the terminal grid will render in the resolved JetBrains Mono or user override \
                 while the UI overlay falls back to Bevy's bundled FiraMono — \
                 expect a visible typeface mismatch between the grid and chrome"
            );
            Handle::<Font>::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configs::OzmuxConfigsPlugin;
    use ab_glyph::Font as AbFont;
    use bevy::asset::AssetPlugin;
    use bevy::text::TextPlugin;
    use bevy::window::{PrimaryWindow, Window, WindowResolution};
    use ozma_tty_renderer::TerminalFontPlugin;
    use ozma_tty_renderer::bundled;
    use std::io::Write;

    /// RAII guard for a process-environment variable. Constructing it via
    /// `EnvVarGuard::set(...)` sets the variable; dropping it removes
    /// it. The Drop runs even on panic, so a test that panics inside
    /// `app.update()` no longer leaks the stale env var into the next
    /// test (which would then run against a misconfigured `OZMUX_CONFIG`
    /// after recovering from the poisoned `env_guard` mutex).
    ///
    /// The caller MUST hold `crate::configs::env_guard()` for the full
    /// lifetime of every `EnvVarGuard` to keep env mutations serialized
    /// across tests.
    struct EnvVarGuard {
        key: &'static str,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            // SAFETY: caller holds env_guard for the duration of this guard.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key }
        }

        fn unset(key: &'static str) -> Self {
            // SAFETY: caller holds env_guard for the duration of this guard.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: caller still holds env_guard (drop runs before the
            // env_guard MutexGuard because of LIFO drop order within
            // each test scope).
            unsafe {
                std::env::remove_var(self.key);
            }
        }
    }

    fn make_test_app() -> (App, std::sync::MutexGuard<'static, ()>, EnvVarGuard) {
        let guard = crate::configs::env_guard();
        let env = EnvVarGuard::unset("OZMUX_CONFIG");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(TextPlugin)
            .init_asset::<Font>();
        // Spawn a PrimaryWindow so init_cell_metrics_from_primary_window
        // (which is registered as a Startup system by TerminalFontPlugin)
        // can run without panicking. We don't actually exercise the
        // metrics in these tests — just need the system not to skip.
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        (app, guard, env)
    }

    #[test]
    fn default_config_keeps_bundled_jbm_in_terminal_fonts() {
        let (mut app, _guard, _env) = make_test_app();
        app.update();
        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.regular.font_data(),
            bundled::REGULAR,
            "regular face is bundled JetBrains Mono when no override is configured"
        );
    }

    #[test]
    fn default_config_inserts_terminal_ui_font_with_strong_handle() {
        let (mut app, _guard, _env) = make_test_app();
        app.update();
        let ui_font = app
            .world()
            .get_resource::<TerminalUiFont>()
            .expect("TerminalUiFont must be inserted on the cold path");
        // A strong handle from Assets::add stores the asset under the
        // returned handle. Look it up to verify it resolves.
        let assets = app.world().resource::<Assets<Font>>();
        assert!(
            assets.get(&ui_font.0).is_some(),
            "TerminalUiFont handle must resolve to an asset stored in Assets<Font>"
        );
    }

    #[test]
    fn configured_normal_path_overrides_regular_face() {
        // Use the bundled JetBrains Mono regular bytes as the "override" — the
        // test verifies the bytes flow through std::fs::read, not that
        // they differ from bundled. Write to a temp file and point
        // OZMUX_CONFIG at a TOML that uses it.
        let tmp_dir = std::env::temp_dir().join("ozmux_font_bridge_test_normal");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let ttf_path = tmp_dir.join("regular.ttf");
        std::fs::write(&ttf_path, bundled::REGULAR).expect("write temp ttf");

        let toml_path = tmp_dir.join("config.toml");
        let mut f = std::fs::File::create(&toml_path).expect("create toml");
        writeln!(f, "[font]\nnormal = \"{}\"\n", ttf_path.to_string_lossy()).expect("write toml");
        drop(f);

        let _guard = crate::configs::env_guard();
        let _env = EnvVarGuard::set("OZMUX_CONFIG", &toml_path);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(TextPlugin)
            .init_asset::<Font>();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        app.update();

        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.regular.font_data(),
            bundled::REGULAR,
            "regular face bytes equal the override file (which happens to contain bundled bytes)"
        );

        let _ = std::fs::remove_file(&ttf_path);
        let _ = std::fs::remove_file(&toml_path);
    }

    #[test]
    fn missing_normal_path_falls_back_to_bundled() {
        let nonexistent = std::env::temp_dir().join("ozmux_font_bridge_test_missing.ttf");
        let _ = std::fs::remove_file(&nonexistent);
        let toml = std::env::temp_dir().join("ozmux_font_bridge_test_missing.toml");
        std::fs::write(
            &toml,
            format!("[font]\nnormal = \"{}\"\n", nonexistent.to_string_lossy()),
        )
        .expect("write toml");

        let _guard = crate::configs::env_guard();
        let _env = EnvVarGuard::set("OZMUX_CONFIG", &toml);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(TextPlugin)
            .init_asset::<Font>();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        app.update();

        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.regular.font_data(),
            bundled::REGULAR,
            "regular face falls back to bundled when configured path does not exist"
        );

        let _ = std::fs::remove_file(&toml);
    }

    #[test]
    fn normal_path_set_does_not_inherit_to_bold() {
        let tmp_dir = std::env::temp_dir().join("ozmux_font_bridge_test_no_inherit");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let normal_path = tmp_dir.join("only-regular.ttf");
        std::fs::write(&normal_path, bundled::REGULAR).expect("write");
        let toml = tmp_dir.join("config.toml");
        std::fs::write(
            &toml,
            format!("[font]\nnormal = \"{}\"\n", normal_path.to_string_lossy()),
        )
        .expect("write toml");

        let _guard = crate::configs::env_guard();
        let _env = EnvVarGuard::set("OZMUX_CONFIG", &toml);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(TextPlugin)
            .init_asset::<Font>();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        app.update();

        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.bold.font_data(),
            bundled::BOLD,
            "bold face must NOT inherit from normal_path; it stays bundled bold"
        );

        let _ = std::fs::remove_file(&normal_path);
        let _ = std::fs::remove_file(&toml);
    }

    #[test]
    fn cjk_fallback_registered_in_cosmic_fontdb() {
        let (mut app, _guard, _env) = make_test_app();
        app.update();

        let font_system = app.world().resource::<bevy::text::CosmicFontSystem>();
        let has_udev_face = font_system
            .db()
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| name.contains("UDEV")));
        assert!(
            has_udev_face,
            "UDEVGothic35 must be registered in cosmic-text fontdb after Startup",
        );
    }

    #[test]
    fn bridge_sets_terminal_font_size_from_default_config() {
        let (mut app, _guard, _env) = make_test_app();
        app.update();
        let size = app.world().resource::<TerminalFontSize>();
        assert_eq!(
            size.0, 11.25,
            "default config font.size (11.25) must reach TerminalFontSize"
        );
    }

    #[test]
    fn bridge_sets_terminal_font_size_from_config_override_without_font_paths() {
        let tmp = std::env::temp_dir().join("ozmux_font_size_override.toml");
        std::fs::write(&tmp, "[font]\nsize = 16.0\n").expect("write toml");

        let _guard = crate::configs::env_guard();
        let _env = EnvVarGuard::set("OZMUX_CONFIG", &tmp);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(TextPlugin)
            .init_asset::<Font>();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        app.update();

        let size = app.world().resource::<TerminalFontSize>();
        assert_eq!(
            size.0, 16.0,
            "config size=16 must reach TerminalFontSize even with no font override (cold path)"
        );

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn powerline_font_resource_is_inserted_and_resolves() {
        let (mut app, _guard, _env) = make_test_app();
        app.update();

        let handle = app
            .world()
            .get_resource::<PowerlineFont>()
            .expect("PowerlineFont must be inserted at Startup")
            .0
            .clone();
        assert!(
            app.world()
                .resource::<Assets<Font>>()
                .get(&handle)
                .is_some(),
            "PowerlineFont handle must resolve to a Font asset"
        );
    }

    #[test]
    fn corrupt_bold_path_falls_back_per_face_without_dropping_normal_override() {
        // Write a corrupt "bold" TTF (just random bytes that won't parse)
        // and a valid "normal" TTF (bundled JetBrains Mono regular). Verify
        // bridge_font_config keeps the normal override AND replaces only
        // the corrupt bold with bundled bold.
        let tmp_dir = std::env::temp_dir().join("ozmux_font_bridge_test_corrupt_bold");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let normal_path = tmp_dir.join("regular.ttf");
        std::fs::write(&normal_path, bundled::REGULAR).expect("write regular");
        let bold_path = tmp_dir.join("bold.ttf");
        std::fs::write(&bold_path, b"this is not a valid TTF").expect("write corrupt bold");
        let toml = tmp_dir.join("config.toml");
        std::fs::write(
            &toml,
            format!(
                "[font]\nnormal = \"{}\"\nbold = \"{}\"\n",
                normal_path.to_string_lossy(),
                bold_path.to_string_lossy(),
            ),
        )
        .expect("write toml");

        let _guard = crate::configs::env_guard();
        let _env = EnvVarGuard::set("OZMUX_CONFIG", &toml);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(TextPlugin)
            .init_asset::<Font>();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        app.update();

        let fonts = app.world().resource::<TerminalFonts>();
        // Regular must remain the user-supplied (= bundled-regular bytes
        // here, but reached via the override path).
        assert_eq!(
            fonts.regular.font_data(),
            bundled::REGULAR,
            "normal override must survive a corrupt bold override"
        );
        // Bold must fall back to bundled because the user-supplied bold
        // is corrupt.
        assert_eq!(
            fonts.bold.font_data(),
            bundled::BOLD,
            "corrupt bold override must fall back to bundled bold"
        );

        let _ = std::fs::remove_file(&normal_path);
        let _ = std::fs::remove_file(&bold_path);
        let _ = std::fs::remove_file(&toml);
    }
}
