//! The bind group that hands the terminal material's uniforms, buffers and
//! textures to the shader.

use crate::material::{OVERLAY_SLOTS, TerminalUiMaterial, params::TerminalParams};
use bevy::{
    ecs::system::{SystemParamItem, lifetimeless::SRes},
    render::{
        render_asset::RenderAssets,
        render_resource::{
            AsBindGroup, AsBindGroupError, BindGroupLayout, BindGroupLayoutEntry, BindingResources,
            BindingType, BufferBindingType, BufferInitDescriptor, BufferUsages,
            OwnedBindingResource, SamplerBindingType, ShaderStages, ShaderType, TextureSampleType,
            TextureViewDimension, UnpreparedBindGroup, encase::UniformBuffer,
        },
        renderer::RenderDevice,
        storage::GpuShaderBuffer,
        texture::{FallbackImage, GpuImage},
    },
};

/// First `@binding` index of the overlay texture array; slot `i` binds at
/// `OVERLAY_TEX_BINDING_BASE + i`. Bindings 0..=5 are params/cells/glyphs/atlas
/// texture/atlas sampler/shared overlay sampler.
const OVERLAY_TEX_BINDING_BASE: u32 = 6;

impl AsBindGroup for TerminalUiMaterial {
    type Data = ();
    type Param = (
        SRes<RenderAssets<GpuImage>>,
        SRes<FallbackImage>,
        SRes<RenderAssets<GpuShaderBuffer>>,
    );

    fn label() -> &'static str {
        "terminal_ui_material"
    }

    fn bind_group_data(&self) -> Self::Data {}

    fn unprepared_bind_group(
        &self,
        _layout: &BindGroupLayout,
        render_device: &RenderDevice,
        (images, fallback_image, storage_buffers): &mut SystemParamItem<'_, '_, Self::Param>,
        _force_no_bindless: bool,
    ) -> Result<UnpreparedBindGroup, AsBindGroupError> {
        let mut params_buffer = UniformBuffer::new(Vec::new());
        params_buffer.write(&self.params).unwrap();

        let cells = storage_buffers
            .get(&self.cells)
            .ok_or(AsBindGroupError::RetryNextUpdate)?;
        let glyphs = storage_buffers
            .get(&self.glyphs)
            .ok_or(AsBindGroupError::RetryNextUpdate)?;
        let atlas = images
            .get(&self.atlas)
            .ok_or(AsBindGroupError::RetryNextUpdate)?;

        let mut bindings = vec![
            (
                0,
                OwnedBindingResource::Buffer(render_device.create_buffer_with_data(
                    &BufferInitDescriptor {
                        label: Some("terminal_params"),
                        usage: BufferUsages::UNIFORM,
                        contents: params_buffer.as_ref(),
                    },
                )),
            ),
            (1, OwnedBindingResource::Buffer(cells.buffer.clone())),
            (2, OwnedBindingResource::Buffer(glyphs.buffer.clone())),
            (
                3,
                OwnedBindingResource::TextureView(
                    TextureViewDimension::D2,
                    atlas.texture_view.clone(),
                ),
            ),
            (
                4,
                OwnedBindingResource::Sampler(SamplerBindingType::Filtering, atlas.sampler.clone()),
            ),
            (
                5,
                OwnedBindingResource::Sampler(
                    SamplerBindingType::Filtering,
                    fallback_image.d2.sampler.clone(),
                ),
            ),
        ];
        bindings.reserve(OVERLAY_SLOTS);

        // NOTE: A `None` slot, OR a `Some` whose GpuImage is not yet loaded,
        // binds the fallback view. The shader never samples it (the `rect.z==0`
        // sentinel gates it), and this avoids stalling the whole material's bind
        // group on a single mid-load overlay texture.
        for (i, handle) in self.overlays.iter().enumerate() {
            let view = handle.as_ref().and_then(|h| images.get(h)).map_or_else(
                || fallback_image.d2.texture_view.clone(),
                |img| img.texture_view.clone(),
            );
            bindings.push((
                OVERLAY_TEX_BINDING_BASE + i as u32,
                OwnedBindingResource::TextureView(TextureViewDimension::D2, view),
            ));
        }

        Ok(UnpreparedBindGroup {
            bindings: BindingResources(bindings),
        })
    }

    fn bind_group_layout_entries(
        _render_device: &RenderDevice,
        _force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry> {
        let texture = |binding: u32| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler = |binding: u32| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Sampler(SamplerBindingType::Filtering),
            count: None,
        };
        let storage = |binding: u32| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        let mut entries = vec![
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: Some(<TerminalParams as ShaderType>::min_size()),
                },
                count: None,
            },
            storage(1),
            storage(2),
            texture(3),
            sampler(4),
            sampler(5),
        ];
        for i in 0..OVERLAY_SLOTS as u32 {
            entries.push(texture(OVERLAY_TEX_BINDING_BASE + i));
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wgsl_overlay_bindings_track_overlay_slots() {
        let src = include_str!("../shaders/terminal_ui_material.wgsl");
        let texture_decls = src.matches("_tex: texture_2d<f32>").count();
        assert_eq!(
            texture_decls,
            OVERLAY_SLOTS + 1,
            "expected atlas + {OVERLAY_SLOTS} overlay texture declarations"
        );
        // NOTE: a stale rect-array size is a silent uniform-offset bug — no
        // binding error, but the shader misreads overlay_dim/overlay_desaturate
        // at the wrong offsets (192/196 instead of 320/324).
        assert!(
            src.contains(&format!("overlay_rects: array<vec4<i32>, {OVERLAY_SLOTS}>")),
            "overlay_rects must be array<vec4<i32>, {OVERLAY_SLOTS}>"
        );
        let calls = src.matches("color = sample_overlay_slot(").count();
        assert_eq!(calls, OVERLAY_SLOTS, "sample_overlay_slot call count");

        // NOTE: the counts above cannot catch a binding-number drift or a
        // copy-paste that samples the wrong texture for a slot — both produce a
        // runtime bind-group/pipeline error, never a test failure. Pin the
        // shared sampler binding, each slot's texture binding, and the per-call
        // overlay_rects[i] <-> overlay{i}_tex pairing, all from the Rust
        // constants, so a drift fails here instead.
        assert!(
            src.contains(&format!(
                "@binding({}) var overlay_samp: sampler;",
                OVERLAY_TEX_BINDING_BASE - 1
            )),
            "shared overlay_samp must be at binding {}",
            OVERLAY_TEX_BINDING_BASE - 1
        );
        for i in 0..OVERLAY_SLOTS {
            let binding = OVERLAY_TEX_BINDING_BASE + i as u32;
            assert!(
                src.contains(&format!(
                    "@binding({binding}) var overlay{i}_tex: texture_2d<f32>;"
                )),
                "overlay{i}_tex must be declared at binding {binding}"
            );
            assert!(
                src.contains(&format!(
                    "sample_overlay_slot(params.overlay_rects[{i}], overlay{i}_tex, overlay_samp,"
                )),
                "slot {i} call must pair overlay_rects[{i}] with overlay{i}_tex"
            );
        }
    }
}
