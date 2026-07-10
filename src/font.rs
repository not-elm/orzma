//! Bridge between `OrzmaConfigsResource.font` and the renderer's
//! `TerminalFonts` Resource, plus the `TerminalUiFont` handle that UI
//! text builders consume. Runs at Startup, ordered
//! `.before(TerminalFontInitSet::InitCellMetrics)` so the renderer's
//! cell-metrics computation sees any overridden font.
//!
//! Startup-only: font changes require a process restart. If a future
//! feature adds config hot-reload, `bridge_font_config` must move to a
//! change-detection system in Update (and additionally re-issue cell
//! metrics + invalidate the glyph atlas — see the renderer crate).

use crate::configs::OrzmaConfigsResource;
use bevy::prelude::*;
use bevy::text::{Font, FontCx, FontSource};
use fontique::{Blob, Collection, Script, SourceCache};
use orzma_configs::font::FontStyleSpec;
use orzma_tty_renderer::{FontFace, TerminalFontInitSet, TerminalFontSize, TerminalFonts, bundled};
use std::str::FromStr;

mod resolve;

/// UI font source (regular face) consumed by UI text builders as
/// `TextFont { font: ui_font.0.clone(), ... }`. Either a `FontSource::Family`
/// (when a system family resolved) or a `FontSource::Handle` to the bundled
/// regular face. Inserted by `bridge_font_config`.
#[derive(Resource, Clone)]
pub struct TerminalUiFont(pub FontSource);

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
                register_cjk_fallback,
            ),
        );
    }
}

/// Registers the bundled CJK fallback font into parley's fontique
/// collection and appends its family to the Han / Hiragana / Katakana
/// script-fallback chains, making it discoverable for spans whose
/// primary `TextFont` lacks CJK coverage.
///
/// NOTE: Bevy's `Assets<Font>::add(...)` is NOT sufficient here —
/// `bevy_text::load_font_assets_into_font_collection` registers every
/// `Font` asset in the collection, but fontique resolves missing glyphs
/// only through per-script fallback chains, which stay empty without
/// system font discovery. `append_fallbacks` is what makes the family
/// reachable. Conversely, that same bevy_text system CLEARS the
/// collection (dropping this registration and its fallback chains) if
/// any `Font` asset is ever removed — orzma never removes font assets
/// after Startup; re-run this registration if that changes.
fn register_cjk_fallback(mut font_cx: ResMut<FontCx>) {
    let blob = Blob::new(
        std::sync::Arc::new(orzma_tty_renderer::bundled::FALLBACK_REGULAR)
            as std::sync::Arc<dyn AsRef<[u8]> + Send + Sync>,
    );
    let registered = font_cx.collection.register_fonts(blob, None);
    let family_ids: Vec<_> = registered.iter().map(|(id, _)| *id).collect();
    for script in [
        Script::from_str_unchecked("Hani"),
        Script::from_str_unchecked("Hira"),
        Script::from_str_unchecked("Kana"),
    ] {
        font_cx
            .collection
            .append_fallbacks(script, family_ids.iter().copied());
    }
    tracing::info!(
        target: "orzma::font",
        "registered UDEVGothic35-Regular into the parley font collection as CJK fallback",
    );
}

fn bridge_font_config(
    mut commands: Commands,
    mut fonts_assets: ResMut<Assets<Font>>,
    mut terminal_fonts: ResMut<TerminalFonts>,
    mut font_size: ResMut<TerminalFontSize>,
    mut font_cx: ResMut<FontCx>,
    configs: Res<OrzmaConfigsResource>,
) {
    font_size.0 = configs.font.size;
    let font = &configs.font;

    let powerline = fonts_assets.add(Font::from_bytes(bundled::REGULAR.to_vec()));
    commands.insert_resource(PowerlineFont(powerline.clone()));

    let no_family = font.faces().iter().all(|(_, face)| face.family.is_none());
    if no_family {
        commands.insert_resource(TerminalUiFont(FontSource::Handle(powerline)));
        return;
    }

    let regular_family = font.normal.family.as_deref();
    let bold_family = font.bold.family.as_deref().or(regular_family);
    let italic_family = font.italic.family.as_deref().or(regular_family);
    let bold_italic_family = font.bold_italic.family.as_deref().or(regular_family);

    // NOTE: materialize &mut FontContext once so `collection` and `source_cache`
    // borrow as disjoint places. Going through `DerefMut` on `font_cx` per call
    // instead would borrow all of FontCx twice and fail to compile.
    let cx = &mut **font_cx;
    let [regular, bold, italic, bold_italic] = [
        (
            regular_family,
            font.normal.style.as_deref(),
            FontFace::Regular,
            bundled::REGULAR,
        ),
        (
            bold_family,
            font.bold.style.as_deref(),
            FontFace::Bold,
            bundled::BOLD,
        ),
        (
            italic_family,
            font.italic.style.as_deref(),
            FontFace::Italic,
            bundled::ITALIC,
        ),
        (
            bold_italic_family,
            font.bold_italic.style.as_deref(),
            FontFace::BoldItalic,
            bundled::BOLD_ITALIC,
        ),
    ]
    .map(|(family, style, face, bundled)| {
        ResolvedFace::resolve(
            &mut cx.collection,
            &mut cx.source_cache,
            family,
            style,
            face,
            bundled,
        )
    });

    let regular_from_family = regular.from_family;
    let new_fonts = TerminalFonts::from_faces(
        (regular.bytes, regular.index),
        (bold.bytes, bold.index),
        (italic.bytes, italic.index),
        (bold_italic.bytes, bold_italic.index),
        bundled::FALLBACK_REGULAR.to_vec(),
        bundled::FALLBACK_BOLD.to_vec(),
        bundled::FALLBACK_ITALIC.to_vec(),
        bundled::FALLBACK_BOLD_ITALIC.to_vec(),
    )
    .expect("validated bytes must parse");
    *terminal_fonts = new_fonts;

    let ui_font = match (regular_from_family, regular_family) {
        (true, Some(family)) => FontSource::Family(family.into()),
        _ => FontSource::Handle(powerline),
    };
    commands.insert_resource(TerminalUiFont(ui_font));
}

/// One primary face resolved for the terminal grid: its bytes, `.ttc` index,
/// and whether it came from a system family (vs a bundled fallback).
struct ResolvedFace {
    bytes: Vec<u8>,
    index: u32,
    from_family: bool,
}

impl ResolvedFace {
    /// Resolves one face: a configured `family` at its `style` (or the face's
    /// default attributes when `style` is omitted) through the system font
    /// collection. A face with no configured family uses `bundled`; a configured
    /// family that cannot be resolved aborts startup (no silent fallback).
    fn resolve(
        collection: &mut Collection,
        source_cache: &mut SourceCache,
        family: Option<&str>,
        style: Option<&str>,
        face: FontFace,
        bundled: &'static [u8],
    ) -> Self {
        let Some(family) = family else {
            return Self::bundled(bundled);
        };
        // NOTE: style was validated at config load; a parse here cannot fail. If
        // a future change loosens `validate()`'s style check, this `.expect()`
        // starts panicking at Startup instead of returning InvalidFontStyle
        // before the app ever gets here.
        let attributes = match style {
            Some(s) => resolve::attributes_of(
                FontStyleSpec::from_str(s).expect("style validated at config load"),
            ),
            None => resolve::face_attributes(face),
        };
        match resolve::resolve_configured_face(collection, source_cache, family, attributes) {
            Ok((bytes, index)) => Self {
                bytes,
                index,
                from_family: true,
            },
            Err(resolve::FamilyNotFound) => panic!(
                "orzma: font family {family:?} configured for the {face:?} face was not found; install it or fix [font] in your config",
            ),
        }
    }

    /// Bundled fallback for one face: the bundled bytes at index 0, marked as not
    /// resolved from a family.
    fn bundled(bundled: &[u8]) -> Self {
        Self {
            bytes: bundled.to_vec(),
            index: 0,
            from_family: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configs::OrzmaConfigsPlugin;
    use ab_glyph::Font as AbFont;
    use bevy::asset::AssetPlugin;
    use bevy::text::TextPlugin;
    use bevy::window::{PrimaryWindow, Window, WindowResolution};
    use fontique::{FontInfoOverride, FontWeight};
    use orzma_tty_renderer::TerminalFontPlugin;
    use orzma_tty_renderer::bundled;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// RAII guard for a process-environment variable. Constructing it via
    /// `EnvVarGuard::set(...)` sets the variable; dropping it restores
    /// whatever value the variable held before construction (or removes it,
    /// if it was unset). The Drop runs even on panic, so a test that panics
    /// inside `app.update()` no longer leaks a stale-set or stale-unset env
    /// var into the next test (which would then run against a
    /// misconfigured `ORZMA_CONFIG` / `$HOME` after recovering from the
    /// poisoned `env_guard` mutex).
    ///
    /// The caller MUST hold `crate::configs::env_guard()` for the full
    /// lifetime of every `EnvVarGuard` to keep env mutations serialized
    /// across tests.
    struct EnvVarGuard {
        key: &'static str,
        prior: Option<std::ffi::OsString>,
    }

    /// Bundles every env override `make_test_app` needs, plus the backing
    /// `TempDir`, so a single value keeps them all alive for the test.
    ///
    /// # Invariants
    ///
    /// `$HOME` / `$XDG_CONFIG_HOME` MUST be redirected into `_tempdir`
    /// alongside `$ORZMA_CONFIG`: `OrzmaConfigsPlugin::build` now runs the
    /// one-time legacy-config migration (`crate::configs::migrate`), which
    /// resolves both the legacy config path and the `bevy::settings`
    /// preferences directory from the real `$HOME` / `$XDG_CONFIG_HOME` when
    /// left unset. Without this redirection, a test that points
    /// `$ORZMA_CONFIG` at an existing temp file would have that file treated
    /// as "the legacy config to migrate now" and PERSISTED — for real — to
    /// the developer's actual `~/Library/Preferences/orzma/` (or
    /// `~/.config/orzma/` on Linux/CI), and every later test in the same
    /// process would then load that leftover file, silently corrupting
    /// their expected defaults.
    struct TestEnvIsolation {
        _tempdir: TempDir,
        _home: EnvVarGuard,
        _xdg_config_home: EnvVarGuard,
        _orzma_config: EnvVarGuard,
    }

    impl TestEnvIsolation {
        fn new(config: Option<&std::path::Path>) -> Self {
            let tempdir = TempDir::new().expect("create isolated $HOME for font test");
            let home = EnvVarGuard::set("HOME", tempdir.path());
            let xdg_config_home =
                EnvVarGuard::set("XDG_CONFIG_HOME", tempdir.path().join(".config"));
            let orzma_config = match config {
                Some(path) => EnvVarGuard::set("ORZMA_CONFIG", path),
                None => EnvVarGuard::unset("ORZMA_CONFIG"),
            };
            TestEnvIsolation {
                _tempdir: tempdir,
                _home: home,
                _xdg_config_home: xdg_config_home,
                _orzma_config: orzma_config,
            }
        }
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prior = std::env::var_os(key);
            // SAFETY: caller holds env_guard for the duration of this guard.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, prior }
        }

        fn unset(key: &'static str) -> Self {
            let prior = std::env::var_os(key);
            // SAFETY: caller holds env_guard for the duration of this guard.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, prior }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: caller still holds env_guard (drop runs before the
            // env_guard MutexGuard because of LIFO drop order within
            // each test scope).
            unsafe {
                match self.prior.take() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    fn make_test_app(
        config: Option<&std::path::Path>,
    ) -> (App, std::sync::MutexGuard<'static, ()>, TestEnvIsolation) {
        let guard = crate::configs::env_guard();
        let env = TestEnvIsolation::new(config);
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
        app.add_plugins(OrzmaConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        (app, guard, env)
    }

    #[test]
    fn default_config_keeps_bundled_jbm_in_terminal_fonts() {
        let (mut app, _guard, _env) = make_test_app(None);
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
        let (mut app, _guard, _env) = make_test_app(None);
        app.update();
        let ui_font = app
            .world()
            .get_resource::<TerminalUiFont>()
            .expect("TerminalUiFont must be inserted on the cold path");
        let FontSource::Handle(handle) = &ui_font.0 else {
            panic!(
                "cold path must insert a FontSource::Handle, got {:?}",
                ui_font.0
            );
        };
        // A strong handle from Assets::add stores the asset under the
        // returned handle. Look it up to verify it resolves.
        let assets = app.world().resource::<Assets<Font>>();
        assert!(
            assets.get(handle).is_some(),
            "TerminalUiFont handle must resolve to an asset stored in Assets<Font>"
        );
    }

    #[test]
    fn cjk_fallback_registered_in_font_collection() {
        let (mut app, _guard, _env) = make_test_app(None);
        app.update();

        let mut font_cx = app.world_mut().resource_mut::<FontCx>();
        let has_udev_family = font_cx
            .collection
            .family_names()
            .any(|name| name.contains("UDEV"));
        assert!(
            has_udev_family,
            "UDEVGothic35 must be registered in the parley font collection after Startup",
        );
        let hira_chain_nonempty = font_cx
            .collection
            .fallback_families(Script::from_str_unchecked("Hira"))
            .next()
            .is_some();
        assert!(
            hira_chain_nonempty,
            "the Hiragana script-fallback chain must contain the bundled CJK family",
        );
    }

    #[test]
    fn bridge_sets_terminal_font_size_from_default_config() {
        let (mut app, _guard, _env) = make_test_app(None);
        app.update();
        let size = app.world().resource::<TerminalFontSize>();
        assert_eq!(
            size.0, 11.25,
            "default config font.size (11.25) must reach TerminalFontSize"
        );
    }

    #[test]
    fn bridge_sets_terminal_font_size_from_config_override_without_font_paths() {
        let tmp = std::env::temp_dir().join("orzma_font_size_override.toml");
        std::fs::write(&tmp, "[font]\nsize = 16.0\n").expect("write toml");

        let (mut app, _guard, _env) = make_test_app(Some(&tmp));
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
        let (mut app, _guard, _env) = make_test_app(None);
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
    #[should_panic(expected = "was not found")]
    fn configured_absent_family_aborts_startup() {
        let tmp = std::env::temp_dir().join("orzma_font_absent_family.toml");
        std::fs::write(&tmp, "[font.normal]\nfamily = \"no-such-family-8b3d2\"\n")
            .expect("write toml");
        let (mut app, _guard, _env) = make_test_app(Some(&tmp));
        app.update();
        let _ = std::fs::remove_file(&tmp);
    }

    /// Registers the bundled JBM regular/bold faces into the test app's
    /// `FontCx` collection under known family names BEFORE `app.update()`
    /// runs `bridge_font_config`, so the resolution is deterministic and
    /// does not depend on any host-installed font.
    #[test]
    fn configured_family_resolves_terminal_fonts_ui_font_and_bold_override() {
        let tmp = std::env::temp_dir().join("orzma_font_family_success_path.toml");
        std::fs::write(
            &tmp,
            "[font.normal]\nfamily = \"OrzmaTestMono\"\n[font.bold]\nfamily = \"OrzmaTestMonoBold\"\n",
        )
        .expect("write toml");

        let (mut app, _guard, _env) = make_test_app(Some(&tmp));

        {
            let mut font_cx = app.world_mut().resource_mut::<FontCx>();
            let regular_blob =
                Blob::new(Arc::new(bundled::REGULAR) as Arc<dyn AsRef<[u8]> + Send + Sync>);
            font_cx.collection.register_fonts(
                regular_blob,
                Some(FontInfoOverride {
                    family_name: Some("OrzmaTestMono"),
                    ..Default::default()
                }),
            );
            let bold_blob =
                Blob::new(Arc::new(bundled::BOLD) as Arc<dyn AsRef<[u8]> + Send + Sync>);
            font_cx.collection.register_fonts(
                bold_blob,
                Some(FontInfoOverride {
                    family_name: Some("OrzmaTestMonoBold"),
                    ..Default::default()
                }),
            );
        }

        app.update();

        let ui_font = app
            .world()
            .get_resource::<TerminalUiFont>()
            .expect("TerminalUiFont must be inserted when the configured family resolves");
        assert_eq!(
            ui_font.0,
            FontSource::Family("OrzmaTestMono".into()),
            "a family that resolves must produce FontSource::Family, not the bundled handle"
        );

        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.regular.font_data(),
            bundled::REGULAR,
            "regular face must resolve to the bytes registered under `normal`"
        );
        assert_eq!(
            fonts.bold.font_data(),
            bundled::BOLD,
            "bold face must resolve via `bold`, not fall back to `normal`'s bytes"
        );
        assert_eq!(
            fonts.italic.font_data(),
            bundled::REGULAR,
            "italic face (no italic override) must fall back to `normal` via .or()"
        );

        let _ = std::fs::remove_file(&tmp);
    }

    /// Registers TWO faces (weight 400 and weight 700) under the SAME family
    /// name, then configures `normal.style = "Bold"`. A single-face family
    /// would resolve regardless of style, so this proves the style-derived
    /// attributes actually drove weight selection: the `normal` slot must
    /// pick the weight-700 face, not the family's first-registered weight-400
    /// face.
    #[test]
    fn configured_style_selects_weight_within_same_family() {
        let tmp = std::env::temp_dir().join("orzma_font_style_selects_weight.toml");
        std::fs::write(
            &tmp,
            "[font.normal]\nfamily = \"OrzmaWeighted\"\nstyle = \"Bold\"\n",
        )
        .expect("write toml");

        let (mut app, _guard, _env) = make_test_app(Some(&tmp));

        {
            let mut font_cx = app.world_mut().resource_mut::<FontCx>();
            let regular_blob =
                Blob::new(Arc::new(bundled::REGULAR) as Arc<dyn AsRef<[u8]> + Send + Sync>);
            font_cx.collection.register_fonts(
                regular_blob,
                Some(FontInfoOverride {
                    family_name: Some("OrzmaWeighted"),
                    weight: Some(FontWeight::new(400.0)),
                    ..Default::default()
                }),
            );
            let bold_blob =
                Blob::new(Arc::new(bundled::BOLD) as Arc<dyn AsRef<[u8]> + Send + Sync>);
            font_cx.collection.register_fonts(
                bold_blob,
                Some(FontInfoOverride {
                    family_name: Some("OrzmaWeighted"),
                    weight: Some(FontWeight::new(700.0)),
                    ..Default::default()
                }),
            );
        }

        app.update();

        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.regular.font_data(),
            bundled::BOLD,
            "style = \"Bold\" must select the weight-700 face, not the family's weight-400 face"
        );

        let _ = std::fs::remove_file(&tmp);
    }
}
