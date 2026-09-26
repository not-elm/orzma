//! Per-pane material uniforms: writes every terminal's `TerminalParams` and
//! overlay textures on each run while a primary window exists.

use crate::{
    cursor::{CaretPaint, CaretPaintInput, CaretStyle, LastKeyInstant, blink_phase_on},
    font::TerminalCellMetricsResource,
    glyph::GlyphAtlas,
    grid::{TerminalCells, TerminalView},
    hyperlink::HyperlinkHoverState,
    material::{
        OVERLAY_SLOTS, TerminalOverlays, TerminalUiMaterial,
        params::{PaneTreatment, TerminalParams},
    },
    pane_style::PaneInactiveStyle,
    system_set::MaterialStage,
};
use bevy::{prelude::*, window::PrimaryWindow};

/// Padding colour used for the area outside a terminal grid (and the whole
/// quad while a grid is unpainted) when the terminal's default background
/// is black. Defaults to black.
#[derive(Resource, Default)]
pub struct TerminalPaddingFallback(pub [u8; 3]);

/// Registers the per-pane uniform write.
pub(crate) struct TerminalParamsPlugin;

impl Plugin for TerminalParamsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerminalPaddingFallback>().add_systems(
            PostUpdate,
            write_terminal_params
                .in_set(MaterialStage::Params)
                .run_if(resource_exists::<TerminalCellMetricsResource>),
        );
    }
}

/// Writes every pane's uniforms and overlay textures into its material; a
/// pane without `TerminalOverlays` binds no overlay texture.
fn write_terminal_params(
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    terminals: Query<(
        Entity,
        &MaterialNode<TerminalUiMaterial>,
        &TerminalCells,
        &TerminalView,
        Option<&PaneInactiveStyle>,
        Option<&TerminalOverlays>,
    )>,
    window: Single<&Window, With<PrimaryWindow>>,
    atlas: Res<GlyphAtlas>,
    metrics: Res<TerminalCellMetricsResource>,
    cursor_config: Res<CaretStyle>,
    last_key: Res<LastKeyInstant>,
    time: Res<Time<Real>>,
    hover: Res<HyperlinkHoverState>,
    fallback: Res<TerminalPaddingFallback>,
) {
    // NOTE: Every run rewrites each pane's `params`, which marks the material
    // asset modified so the render world rebuilds its bind group. Only that
    // rebuild repoints the bind group at a cell or glyph `ShaderBuffer` whose
    // size changed (bevy_render then allocates a new GPU buffer) and at a
    // webview overlay texture that a resize re-created. Writing only when
    // the uniforms change would leave the bind group on the old buffers and
    // textures.
    let phase_on = blink_phase_on(
        time.elapsed().saturating_sub(last_key.0),
        cursor_config.blink_interval,
        cursor_config.blink_timeout,
    );
    let atlas_size_px = Vec2::new(atlas.width() as f32, atlas.height() as f32);
    for (entity, node, cells, view, pane_style, overlays) in &terminals {
        let Some(mut material) = materials.get_mut(&node.0) else {
            continue;
        };
        let (hover_hyperlink_id, hover_active) = match (hover.entity, hover.hyperlink_id) {
            (Some(hovered), Some(id)) if hovered == entity => {
                (id.get(), u32::from(hover.modifier_held))
            }
            _ => (0, 0),
        };
        let caret = CaretPaint::new(
            view.caret(),
            CaretPaintInput {
                suppressed: view.suppress_cursor,
                focused: window.focused && pane_style.is_none(),
                unfocused_hollow: cursor_config.unfocused_hollow,
                phase_on,
            },
        );
        let mut params = TerminalParams::new(
            view,
            &cells.palette,
            &metrics.metrics,
            &PaneTreatment::from_style(pane_style),
            atlas_size_px,
            window.scale_factor(),
            fallback.0,
            hover_hyperlink_id,
            hover_active,
            caret,
            cursor_config.thickness,
        );
        match overlays {
            Some(overlays) => {
                params.overlay_rects = overlays.rects;
                material.set_overlays(&overlays.textures);
            }
            None => material.set_overlays(&[const { None }; OVERLAY_SLOTS]),
        }
        material.params = params;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::TerminalFonts;
    use bevy::asset::uuid_handle;
    use orzma_vt::prelude::HyperlinkId;

    /// An app running only the uniform write, for one pane and no primary
    /// window; returns the pane and its material.
    fn params_app() -> (App, Entity, Handle<TerminalUiMaterial>) {
        let mut app = App::new();
        app.add_plugins(TerminalParamsPlugin)
            .init_resource::<Assets<TerminalUiMaterial>>()
            .init_resource::<GlyphAtlas>()
            .insert_resource(TerminalCellMetricsResource::new(
                &TerminalFonts::default(),
                12,
            ))
            .init_resource::<CaretStyle>()
            .init_resource::<LastKeyInstant>()
            .init_resource::<Time<Real>>()
            .init_resource::<HyperlinkHoverState>()
            .init_resource::<TerminalPaddingFallback>();
        let material = app
            .world_mut()
            .resource_mut::<Assets<TerminalUiMaterial>>()
            .add(TerminalUiMaterial::default());
        let pane = app
            .world_mut()
            .spawn((MaterialNode(material.clone()), TerminalView::default()))
            .id();
        (app, pane, material)
    }

    fn material_of<'a>(
        app: &'a App,
        material: &Handle<TerminalUiMaterial>,
    ) -> &'a TerminalUiMaterial {
        app.world()
            .resource::<Assets<TerminalUiMaterial>>()
            .get(material)
            .expect("the pane's material")
    }

    fn hover_over(app: &mut App, pane: Entity, link: u32) {
        let mut hover = app.world_mut().resource_mut::<HyperlinkHoverState>();
        hover.entity = Some(pane);
        hover.hyperlink_id = HyperlinkId::new(link);
    }

    /// Asserts that the uniforms are written on every update while a
    /// primary window exists, and left as they were while none does.
    ///
    /// Case: the pointer moves across links while the window is being
    /// re-created, and keeps moving after it comes back.
    #[test]
    fn params_follow_their_inputs_only_while_a_primary_window_exists() {
        let (mut app, pane, material) = params_app();
        hover_over(&mut app, pane, 5);
        app.update();
        assert_eq!(
            material_of(&app, &material).params.hover_hyperlink_id,
            0,
            "no primary window, so nothing is written"
        );

        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.update();
        assert_eq!(material_of(&app, &material).params.hover_hyperlink_id, 5);

        hover_over(&mut app, pane, 7);
        app.update();
        assert_eq!(material_of(&app, &material).params.hover_hyperlink_id, 7);
    }

    /// Asserts that a pane without overlays binds no overlay textures,
    /// even after it bound some.
    ///
    /// Case: a host plugin removes a pane's `TerminalOverlays` outright,
    /// instead of resetting its slots, after the pane displayed a webview.
    #[test]
    fn a_pane_without_overlays_binds_no_overlay_textures() {
        const WEBVIEW: Handle<Image> = uuid_handle!("c0fee000-0000-4000-8000-000000000004");
        let (mut app, pane, material) = params_app();
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        let mut overlays = TerminalOverlays::default();
        overlays.textures[0] = Some(WEBVIEW);
        app.world_mut().entity_mut(pane).insert(overlays);
        app.update();
        assert_eq!(material_of(&app, &material).overlays[0], Some(WEBVIEW));

        app.world_mut()
            .entity_mut(pane)
            .remove::<TerminalOverlays>();
        app.update();
        assert!(
            material_of(&app, &material)
                .overlays
                .iter()
                .all(Option::is_none)
        );
    }
}
