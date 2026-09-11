use crate::glyph::{
    atlas::{GlyphAtlas, TerminalGlyphAtlasPlugin},
    font::TerminalFontPlugin,
};
use crate::material::TerminalMaterialSystems;
use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

pub(crate) mod atlas;
pub(crate) mod font;

pub struct TerminalGlyphPlugin;

impl Plugin for TerminalGlyphPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((TerminalGlyphAtlasPlugin, TerminalFontPlugin))
            .add_systems(Startup, init_atlas_image)
            // NOTE: Must run in the same schedule as
            // `update_terminal_material` (now `PostUpdate`) so the `.after`
            // ordering is honoured by Bevy's executor — cross-schedule
            // `.after` is silently ignored.
            .add_systems(
                PostUpdate,
                sync_atlas_image
                    .after(TerminalMaterialSystems::UpdateMaterial)
                    .run_if(resource_changed::<GlyphAtlas>),
            );
    }
}

/// Handle to the GPU-side mirror of the `GlyphAtlas`, plus the last
/// observed atlas generation so the sync system only re-uploads on change.
#[derive(Resource)]
pub struct AtlasImage {
    /// Bevy `Image` asset whose pixel buffer mirrors `GlyphAtlas.pixels`.
    pub handle: Handle<Image>,
    /// Last `GlyphAtlas.generation` seen by `sync_atlas_image`.
    pub last_generation: u64,
}

fn init_atlas_image(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    atlas: Res<GlyphAtlas>,
) {
    let mut image = Image::new(
        Extent3d {
            width: atlas.width(),
            height: atlas.height(),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        expand_r8_to_rgba8(&atlas.pixels),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    let handle = images.add(image);
    commands.insert_resource(AtlasImage {
        handle,
        last_generation: atlas.generation,
    });
}

fn sync_atlas_image(
    mut atlas_image: ResMut<AtlasImage>,
    mut images: ResMut<Assets<Image>>,
    atlas: Res<GlyphAtlas>,
) {
    if atlas.generation == atlas_image.last_generation {
        return;
    }
    if let Some(mut image) = images.get_mut(&atlas_image.handle) {
        image.data = Some(expand_r8_to_rgba8(&atlas.pixels));
    }
    atlas_image.last_generation = atlas.generation;
}

/// Expands an R8 (single-byte coverage) atlas buffer into an RGBA8 buffer
/// with white RGB and the coverage value in the alpha channel — the format
/// the fragment shader samples.
fn expand_r8_to_rgba8(src: &[u8]) -> Vec<u8> {
    let mut dst = Vec::with_capacity(src.len() * 4);
    for &a in src {
        dst.extend_from_slice(&[255, 255, 255, a]);
    }
    dst
}
