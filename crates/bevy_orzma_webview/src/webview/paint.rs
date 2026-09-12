//! Copies the CPU paint frames CEF produces on Windows and Linux into the
//! headless `WebviewTextureTarget` image each mounted webview renders through.

#[cfg(not(target_os = "macos"))]
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
#[cfg(not(target_os = "macos"))]
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
#[cfg(not(target_os = "macos"))]
use bevy_cef::prelude::WebviewTextureTarget;
#[cfg(not(target_os = "macos"))]
use bevy_cef_core::prelude::{RenderPaintElementType, RenderTextureMessage};

/// Registers the paint bridge on the platforms where CEF paints on the CPU.
///
/// On macOS it registers nothing.
pub(crate) struct PaintPlugin;

impl Plugin for PaintPlugin {
    #[cfg(not(target_os = "macos"))]
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            copy_cpu_paints_into_targets.run_if(on_message::<RenderTextureMessage>),
        );
    }

    #[cfg(target_os = "macos")]
    fn build(&self, _app: &mut App) {}
}

/// Copies each view paint into the `WebviewTextureTarget` image of the webview
/// it names: in place when the image already has the paint's size, otherwise by
/// replacing the image with a `Bgra8UnormSrgb` image of that size. Popup paints
/// and paints for entities without a target are ignored.
#[cfg(not(target_os = "macos"))]
fn copy_cpu_paints_into_targets(
    mut images: ResMut<Assets<Image>>,
    mut paints: MessageReader<RenderTextureMessage>,
    targets: Query<&WebviewTextureTarget>,
) {
    for paint in paints.read() {
        if paint.ty != RenderPaintElementType::View {
            continue;
        }
        let Ok(target) = targets.get(paint.webview) else {
            continue;
        };
        let Some(mut image) = images.get_mut(&target.0) else {
            continue;
        };
        let size = image.texture_descriptor.size;
        let same_size = size.width == paint.width && size.height == paint.height;
        let in_place = image
            .data
            .as_mut()
            .filter(|data| same_size && data.len() == paint.buffer.len());
        if let Some(data) = in_place {
            data.copy_from_slice(&paint.buffer);
        } else {
            *image = Image::new(
                Extent3d {
                    width: paint.width,
                    height: paint.height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                paint.buffer.clone(),
                TextureFormat::Bgra8UnormSrgb,
                RenderAssetUsages::all(),
            );
        }
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    use bevy::render::render_resource::TextureFormat;
    use bevy_cef::prelude::WebviewTextureTarget;
    use bevy_cef_core::prelude::{RenderPaintElementType, RenderTextureMessage};

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Assets<Image>>()
            .add_message::<RenderTextureMessage>()
            .add_plugins(PaintPlugin);
        app
    }

    fn spawn_target(app: &mut App) -> (Entity, Handle<Image>) {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        let entity = app
            .world_mut()
            .spawn(WebviewTextureTarget(handle.clone()))
            .id();
        (entity, handle)
    }

    fn paint(
        webview: Entity,
        ty: RenderPaintElementType,
        side: u32,
        fill: u8,
    ) -> RenderTextureMessage {
        RenderTextureMessage {
            webview,
            ty,
            width: side,
            height: side,
            buffer: vec![fill; (side * side * 4) as usize],
        }
    }

    fn image_of(app: &App, handle: &Handle<Image>) -> (u32, u32, TextureFormat, Vec<u8>) {
        let image = app
            .world()
            .resource::<Assets<Image>>()
            .get(handle)
            .expect("the target image exists");
        (
            image.texture_descriptor.size.width,
            image.texture_descriptor.size.height,
            image.texture_descriptor.format,
            image.data.clone().unwrap_or_default(),
        )
    }

    /// Asserts that a view paint for a webview with a texture target replaces
    /// the placeholder with a BGRA image of the paint's size holding its bytes.
    ///
    /// Case: orzmd's page finishes its first layout inside a Windows pane and
    /// CEF delivers the first full-frame paint.
    #[test]
    fn a_view_paint_fills_the_target_image() {
        let mut app = app();
        let (entity, handle) = spawn_target(&mut app);

        app.world_mut()
            .write_message(paint(entity, RenderPaintElementType::View, 2, 0x7f));
        app.update();

        let (w, h, format, data) = image_of(&app, &handle);
        assert_eq!((w, h), (2, 2));
        assert_eq!(format, TextureFormat::Bgra8UnormSrgb);
        assert_eq!(data, vec![0x7f; 16]);
    }

    /// Asserts that a same-size follow-up paint updates the image bytes in
    /// place and keeps its size.
    ///
    /// Case: the page repaints as the user scrolls a rendered document.
    #[test]
    fn a_same_size_paint_updates_the_bytes_in_place() {
        let mut app = app();
        let (entity, handle) = spawn_target(&mut app);
        app.world_mut()
            .write_message(paint(entity, RenderPaintElementType::View, 2, 0x11));
        app.update();

        app.world_mut()
            .write_message(paint(entity, RenderPaintElementType::View, 2, 0x22));
        app.update();

        let (w, h, _, data) = image_of(&app, &handle);
        assert_eq!((w, h), (2, 2));
        assert_eq!(data, vec![0x22; 16]);
    }

    /// Asserts that a popup paint and a paint for an entity without a target
    /// leave every image untouched.
    ///
    /// Case: a page opens a select-box popup, and a stray paint arrives for a
    /// webview that has already been torn down.
    #[test]
    fn popup_and_untargeted_paints_are_ignored() {
        let mut app = app();
        let (entity, handle) = spawn_target(&mut app);
        let before = image_of(&app, &handle);
        let stray = app.world_mut().spawn_empty().id();

        app.world_mut()
            .write_message(paint(entity, RenderPaintElementType::Popup, 2, 0x33));
        app.world_mut()
            .write_message(paint(stray, RenderPaintElementType::View, 2, 0x44));
        app.update();

        assert_eq!(image_of(&app, &handle), before);
    }
}
