//! Bridge between `OzmuxConfigsResource.font` and the renderer's
//! `TerminalFonts` Resource, plus the `TerminalUiFont` handle that UI
//! text builders consume. Runs at Startup, ordered
//! `.before(TerminalFontInitSet::InitCellMetrics)` so the renderer's
//! cell-metrics computation sees any overridden font.

use crate::configs::OzmuxConfigsResource;
use bevy::prelude::*;
use bevy::text::Font;
use bevy_terminal_renderer::{FontFace, TerminalFontInitSet, TerminalFonts};
use ozmux_configs::font::FontConfig;
use ozmux_configs::path::{SystemEnv, expand_user_path};
use std::path::Path;

/// Strong handle to the UI font asset (regular face). Inserted by
/// `bridge_font_config`. UI builders read this resource to set
/// `TextFont { font, ... }`.
#[derive(Resource, Clone)]
pub(crate) struct TerminalUiFont(pub Handle<Font>);

/// Bevy plugin that wires `bridge_font_config` into Startup.
pub(crate) struct FontBridgePlugin;

impl Plugin for FontBridgePlugin {
    fn build(&self, app: &mut App) {
        // NOTE: must run before TerminalFontInitSet::InitCellMetrics so
        // the metrics computed inside the renderer's
        // init_cell_metrics_from_primary_window reflect any user-supplied
        // font override.
        app.add_systems(
            Startup,
            bridge_font_config.before(TerminalFontInitSet::InitCellMetrics),
        );
    }
}

const BUNDLED_REGULAR: &[u8] = include_bytes!(
    "../assets/fonts/iosevka/IosevkaTermNerdFontMono-Regular.ttf"
);
const BUNDLED_BOLD: &[u8] = include_bytes!(
    "../assets/fonts/iosevka/IosevkaTermNerdFontMono-Bold.ttf"
);
const BUNDLED_ITALIC: &[u8] = include_bytes!(
    "../assets/fonts/iosevka/IosevkaTermNerdFontMono-Italic.ttf"
);
const BUNDLED_BOLD_ITALIC: &[u8] = include_bytes!(
    "../assets/fonts/iosevka/IosevkaTermNerdFontMono-BoldItalic.ttf"
);

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

fn bridge_font_config(
    mut commands: Commands,
    configs: Res<OzmuxConfigsResource>,
    mut fonts_assets: ResMut<Assets<Font>>,
    mut terminal_fonts: ResMut<TerminalFonts>,
) {
    let font: &FontConfig = &configs.font;

    // Load per face — independently. Failure to load one face leaves the
    // others untouched.
    let regular_bytes = load_face_bytes(
        font.normal_path.as_deref(),
        BUNDLED_REGULAR,
        FontFace::Regular,
    );
    let bold_bytes =
        load_face_bytes(font.bold_path.as_deref(), BUNDLED_BOLD, FontFace::Bold);
    let italic_bytes = load_face_bytes(
        font.italic_path.as_deref(),
        BUNDLED_ITALIC,
        FontFace::Italic,
    );
    let bold_italic_bytes = load_face_bytes(
        font.bold_italic_path.as_deref(),
        BUNDLED_BOLD_ITALIC,
        FontFace::BoldItalic,
    );

    // Keep a copy of regular bytes for the UI font before moving into TerminalFonts.
    let ui_regular_bytes = regular_bytes.clone();

    // Try to build TerminalFonts; on parse failure for any face, fall
    // back to bundled-only and warn. (Simpler than reconstructing which
    // specific face failed.)
    let new_fonts = match TerminalFonts::from_bytes(
        regular_bytes,
        bold_bytes,
        italic_bytes,
        bold_italic_bytes,
    ) {
        Ok(f) => f,
        Err(err) => {
            tracing::warn!(%err, "TerminalFonts::from_bytes rejected an override face; \
                              substituting bundled bytes for all faces");
            TerminalFonts::from_bytes(
                BUNDLED_REGULAR.to_vec(),
                BUNDLED_BOLD.to_vec(),
                BUNDLED_ITALIC.to_vec(),
                BUNDLED_BOLD_ITALIC.to_vec(),
            )
            .expect("bundled TTFs must parse")
        }
    };
    *terminal_fonts = new_fonts;

    // Insert TerminalUiFont. Use the regular face bytes (already cloned above)
    // so the UI font matches whatever the renderer just adopted.
    let handle = match Font::try_from_bytes(ui_regular_bytes) {
        Ok(font_asset) => fonts_assets.add(font_asset),
        Err(err) => {
            tracing::warn!(%err, "bevy_text::Font::try_from_bytes rejected the regular face; \
                              falling back to Bevy's default font (FiraMono via default_font)");
            Handle::<Font>::default()
        }
    };
    commands.insert_resource(TerminalUiFont(handle));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::Font as AbFont;
    use crate::configs::OzmuxConfigsPlugin;
    use bevy::asset::AssetPlugin;
    use bevy::window::{PrimaryWindow, Window, WindowResolution};
    use bevy_terminal_renderer::TerminalFontPlugin;
    use std::io::Write;

    fn make_test_app() -> (App, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::configs::env_guard();
        // SAFETY: env mutations serialized by env_guard().
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
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
        (app, guard)
    }

    #[test]
    fn default_config_keeps_bundled_iosevka_in_terminal_fonts() {
        let (mut app, _guard) = make_test_app();
        app.update();
        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.regular.font_data(),
            BUNDLED_REGULAR,
            "regular face is bundled Iosevka when no override is configured"
        );
    }

    #[test]
    fn default_config_inserts_terminal_ui_font_with_strong_handle() {
        let (mut app, _guard) = make_test_app();
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
        // Use the bundled Iosevka regular bytes as the "override" — the
        // test verifies the bytes flow through std::fs::read, not that
        // they differ from bundled. Write to a temp file and point
        // OZMUX_CONFIG at a TOML that uses it.
        let tmp_dir = std::env::temp_dir().join("ozmux_font_bridge_test_normal");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let ttf_path = tmp_dir.join("regular.ttf");
        std::fs::write(&ttf_path, BUNDLED_REGULAR).expect("write temp ttf");

        let toml_path = tmp_dir.join("config.toml");
        let mut f = std::fs::File::create(&toml_path).expect("create toml");
        writeln!(
            f,
            "[font.normal]\npath = \"{}\"\n",
            ttf_path.to_string_lossy()
        )
        .expect("write toml");
        drop(f);

        let _guard = crate::configs::env_guard();
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::set_var("OZMUX_CONFIG", &toml_path);
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
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
            BUNDLED_REGULAR,
            "regular face bytes equal the override file (which happens to contain bundled bytes)"
        );

        let _ = std::fs::remove_file(&ttf_path);
        let _ = std::fs::remove_file(&toml_path);
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
    }

    #[test]
    fn missing_normal_path_falls_back_to_bundled() {
        let nonexistent = std::env::temp_dir()
            .join("ozmux_font_bridge_test_missing.ttf");
        let _ = std::fs::remove_file(&nonexistent);
        let toml = std::env::temp_dir()
            .join("ozmux_font_bridge_test_missing.toml");
        std::fs::write(
            &toml,
            format!("[font.normal]\npath = \"{}\"\n", nonexistent.to_string_lossy()),
        )
        .expect("write toml");

        let _guard = crate::configs::env_guard();
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::set_var("OZMUX_CONFIG", &toml);
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
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
            BUNDLED_REGULAR,
            "regular face falls back to bundled when configured path does not exist"
        );

        let _ = std::fs::remove_file(&toml);
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
    }

    #[test]
    fn normal_path_set_does_not_inherit_to_bold() {
        let tmp_dir = std::env::temp_dir().join("ozmux_font_bridge_test_no_inherit");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let normal_path = tmp_dir.join("only-regular.ttf");
        std::fs::write(&normal_path, BUNDLED_REGULAR).expect("write");
        let toml = tmp_dir.join("config.toml");
        std::fs::write(
            &toml,
            format!(
                "[font.normal]\npath = \"{}\"\n",
                normal_path.to_string_lossy()
            ),
        )
        .expect("write toml");

        let _guard = crate::configs::env_guard();
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::set_var("OZMUX_CONFIG", &toml);
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
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
            BUNDLED_BOLD,
            "bold face must NOT inherit from normal_path; it stays bundled bold"
        );

        let _ = std::fs::remove_file(&normal_path);
        let _ = std::fs::remove_file(&toml);
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
    }
}
