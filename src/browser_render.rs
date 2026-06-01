//! Browser activity rendering: a native back/forward/reload + address-bar
//! toolbar over a `bevy_cef` page webview. The activity host (a column) gets
//! two persistent, non-`StructuralNode` children built once — a toolbar and a
//! page-webview node — and (in a later phase) a CEF webview attached to the
//! laid-out page child after host-side omnibox resolution.

use crate::configs::OzmuxConfigsResource;
use crate::ui::{
    AddrBarText, AddressEdit, BrowserActivityMarker, BrowserNavButton, BrowserPageWebview,
    BrowserToolbarState, HostActivityEntity, NavAction, PageWebviewOf,
};
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, JustifyContent, Val};
use bevy_cef::prelude::*;
use ozmux_configs::browser::resolve_omnibox_input;
use ozmux_multiplexer::ActivityKind;

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

/// Attaches the CEF page webview to a laid-out page-webview child once its own
/// `ComputedNode` is real, seeding `WebviewSize` from the child (not the host)
/// so CEF is created at the final page size — no mid-load resize.
fn attach_browser_webview(
    mut commands: Commands,
    mut materials: ResMut<Assets<WebviewUiMaterial>>,
    configs: Res<OzmuxConfigsResource>,
    pages: Query<(Entity, &ComputedNode, &PageWebviewOf), Without<WebviewSource>>,
    hosts: Query<&HostActivityEntity>,
    kinds: Query<&ActivityKind>,
) {
    for (page, computed, owner) in pages.iter() {
        let size = computed.size() * computed.inverse_scale_factor();
        if size.x < 1.0 || size.y < 1.0 {
            continue;
        }
        let Ok(host_activity) = hosts.get(owner.0) else {
            continue;
        };
        let Ok(ActivityKind::Browser { initial_url, .. }) = kinds.get(host_activity.0) else {
            continue;
        };
        let raw = initial_url.as_deref().unwrap_or("");
        let resolved = resolve_omnibox_input(raw, &configs.browser.search_template);
        if resolved.is_empty() {
            continue;
        }
        commands.entity(page).insert((
            WebviewSource::new(resolved),
            WebviewSize(size),
            MaterialNode(materials.add(WebviewUiMaterial::default())),
        ));
    }
}

/// Mirrors a page webview's `AddressChanged` onto its host's `BrowserToolbarState`.
fn on_address_changed(
    ev: On<AddressChanged>,
    owners: Query<&PageWebviewOf>,
    mut states: Query<&mut BrowserToolbarState>,
) {
    let Ok(owner) = owners.get(ev.webview) else {
        return;
    };
    let Ok(mut state) = states.get_mut(owner.0) else {
        return;
    };
    state.url = ev.url.clone();
    state.can_go_back = ev.can_go_back;
    state.can_go_forward = ev.can_go_forward;
}

/// Mirrors a page webview's `LoadingStateChanged` onto its host's `BrowserToolbarState`.
fn on_loading_state_changed(
    ev: On<LoadingStateChanged>,
    owners: Query<&PageWebviewOf>,
    mut states: Query<&mut BrowserToolbarState>,
) {
    let Ok(owner) = owners.get(ev.webview) else {
        return;
    };
    let Ok(mut state) = states.get_mut(owner.0) else {
        return;
    };
    state.is_loading = ev.is_loading;
    state.can_go_back = ev.can_go_back;
    state.can_go_forward = ev.can_go_forward;
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
    use ozmux_multiplexer::MultiplexerPlugin;

    fn make_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(ImagePlugin::default())
            .add_plugins(MultiplexerPlugin)
            .init_asset::<WebviewUiMaterial>()
            .insert_resource(crate::configs::OzmuxConfigsResource(ozmux_configs::OzmuxConfigs::default()));
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

    #[test]
    fn attach_resolves_omnibox_and_seeds_child_size() {
        use crate::ui::HostActivityEntity;
        use ozmux_multiplexer::ActivityKind;
        let mut app = make_test_app();
        app.add_systems(Update, (build_browser_chrome, attach_browser_webview).chain());

        let activity = app
            .world_mut()
            .spawn(ActivityKind::Browser { initial_url: Some("github.com".into()), profile: Default::default() })
            .id();
        let host = app
            .world_mut()
            .spawn((BrowserActivityMarker, HostActivityEntity(activity), laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        // NOTE: first tick builds chrome; attach is a no-op until the page child is laid out.
        app.update();

        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;
        app.world_mut().entity_mut(page).insert(laid_out_node(Vec2::new(800.0, 568.0)));
        // NOTE: page child now has a ComputedNode, so attach fires this tick.
        app.update();

        match app.world().get::<WebviewSource>(page) {
            Some(WebviewSource::Url(url)) => assert_eq!(url, "https://github.com"),
            other => panic!("expected resolved Url, got {other:?}"),
        }
        assert_eq!(
            app.world().get::<WebviewSize>(page).map(|s| s.0),
            Some(Vec2::new(800.0, 568.0)),
            "webview seeded at the CHILD's laid-out size, not the host's"
        );
    }

    #[test]
    fn address_changed_updates_host_toolbar_state() {
        let mut app = make_test_app();
        app.add_systems(Update, build_browser_chrome);
        app.add_observer(on_address_changed);
        let host = app
            .world_mut()
            .spawn((BrowserActivityMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update();
        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;

        app.world_mut().trigger(AddressChanged {
            webview: page,
            url: "https://example.com/x".into(),
            can_go_back: true,
            can_go_forward: false,
        });
        app.world_mut().flush();

        let state = app.world().get::<BrowserToolbarState>(host).unwrap();
        assert_eq!(state.url, "https://example.com/x");
        assert!(state.can_go_back);
        assert!(!state.can_go_forward);
    }

    #[test]
    fn loading_state_changed_updates_host_toolbar_state() {
        let mut app = make_test_app();
        app.add_systems(Update, build_browser_chrome);
        app.add_observer(on_loading_state_changed);
        let host = app
            .world_mut()
            .spawn((BrowserActivityMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update();
        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;

        app.world_mut().trigger(LoadingStateChanged {
            webview: page,
            is_loading: true,
            can_go_back: false,
            can_go_forward: true,
        });
        app.world_mut().flush();

        let state = app.world().get::<BrowserToolbarState>(host).unwrap();
        assert!(state.is_loading);
        assert!(!state.can_go_back);
        assert!(state.can_go_forward);
    }

    #[test]
    fn attach_skips_empty_input() {
        use crate::ui::HostActivityEntity;
        use ozmux_multiplexer::ActivityKind;
        let mut app = make_test_app();
        app.add_systems(Update, (build_browser_chrome, attach_browser_webview).chain());
        let activity = app
            .world_mut()
            .spawn(ActivityKind::Browser { initial_url: None, profile: Default::default() })
            .id();
        let host = app
            .world_mut()
            .spawn((BrowserActivityMarker, HostActivityEntity(activity), laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update();
        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;
        app.world_mut().entity_mut(page).insert(laid_out_node(Vec2::new(800.0, 568.0)));
        app.update();
        assert!(
            app.world().get::<WebviewSource>(page).is_none(),
            "empty initial_url resolves to empty; no webview attached"
        );
    }
}
