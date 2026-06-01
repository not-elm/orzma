//! Browser activity rendering: a native back/forward/reload + address-bar
//! toolbar over a `bevy_cef` page webview. The activity host (a column) gets
//! two persistent, non-`StructuralNode` children built once — a toolbar and a
//! page-webview node — and (in a later phase) a CEF webview attached to the
//! laid-out page child after host-side omnibox resolution.

use crate::ui::{
    AddrBarText, AddressEdit, BrowserActivityMarker, BrowserNavButton, BrowserPageWebview,
    BrowserToolbarState, NavAction, PageWebviewOf,
};
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, JustifyContent, Val};

const TOOLBAR_HEIGHT_PX: f32 = 32.0;

/// Builds the toolbar + empty page-webview children for each laid-out browser
/// host that has not been built yet (no `BrowserPageWebview` pointer).
fn build_browser_chrome(
    mut commands: Commands,
    hosts: Query<
        (Entity, &ComputedNode),
        (With<BrowserActivityMarker>, Without<BrowserPageWebview>),
    >,
) {
    for (host, computed) in hosts.iter() {
        if computed.size().x < 1.0 || computed.size().y < 1.0 {
            continue;
        }

        let back = spawn_nav_button(&mut commands, host, NavAction::Back, "<");
        let forward = spawn_nav_button(&mut commands, host, NavAction::Forward, ">");
        let reload = spawn_nav_button(&mut commands, host, NavAction::Reload, "R");
        let addr = commands
            .spawn((Text::new(""), AddrBarText, Node { flex_grow: 1.0, ..default() }))
            .id();

        let toolbar = commands
            .spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(TOOLBAR_HEIGHT_PX),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::FlexStart,
                    ..default()
                },
                ChildOf(host),
            ))
            .id();
        commands.entity(toolbar).add_children(&[back, forward, reload, addr]);

        let page = commands
            .spawn((
                Node { flex_grow: 1.0, width: Val::Percent(100.0), ..default() },
                PageWebviewOf(host),
                ChildOf(host),
            ))
            .id();

        commands.entity(host).insert((
            BrowserPageWebview(page),
            BrowserToolbarState::default(),
            AddressEdit::default(),
        ));
    }
}

fn spawn_nav_button(commands: &mut Commands, host: Entity, action: NavAction, label: &str) -> Entity {
    commands
        .spawn((
            Button,
            Node {
                width: Val::Px(28.0),
                height: Val::Px(28.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BrowserNavButton { host, action },
        ))
        .with_children(|p| {
            p.spawn(Text::new(label.to_string()));
        })
        .id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy_cef::prelude::*;
    use ozmux_multiplexer::MultiplexerPlugin;

    fn make_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(ImagePlugin::default())
            .add_plugins(MultiplexerPlugin)
            .init_asset::<WebviewUiMaterial>();
        app
    }

    fn laid_out_node(size: Vec2) -> ComputedNode {
        ComputedNode { size, inverse_scale_factor: 1.0, ..ComputedNode::DEFAULT }
    }

    #[test]
    fn build_chrome_spawns_toolbar_and_empty_page_child() {
        let mut app = make_test_app();
        app.add_systems(Update, build_browser_chrome);
        let host = app.world_mut().spawn((BrowserActivityMarker, laid_out_node(Vec2::new(800.0, 600.0)))).id();
        app.update();

        let page = app.world().get::<BrowserPageWebview>(host).expect("host gets BrowserPageWebview").0;
        assert!(app.world().get::<WebviewSource>(page).is_none(), "page child must be an empty Node (no webview yet)");
        assert_eq!(app.world().get::<PageWebviewOf>(page).map(|p| p.0), Some(host), "page child points back to host");
        assert!(app.world().get::<BrowserToolbarState>(host).is_some());
        assert!(app.world().get::<AddressEdit>(host).is_some());
    }

    #[test]
    fn build_chrome_is_idempotent() {
        let mut app = make_test_app();
        app.add_systems(Update, build_browser_chrome);
        let host = app.world_mut().spawn((BrowserActivityMarker, laid_out_node(Vec2::new(800.0, 600.0)))).id();
        app.update();
        let first = app.world().get::<BrowserPageWebview>(host).unwrap().0;
        app.update();
        let second = app.world().get::<BrowserPageWebview>(host).unwrap().0;
        assert_eq!(first, second, "chrome built exactly once");
    }
}
