//! Browser activity rendering: a native back/forward/reload + address-bar
//! toolbar over a `bevy_cef` page webview. The activity host (a column) gets
//! two persistent, non-`StructuralNode` children built once — a toolbar and a
//! page-webview node — and (in a later phase) a CEF webview attached to the
//! laid-out page child after host-side omnibox resolution.

use crate::clipboard::Clipboard;
use crate::configs::OzmuxConfigsResource;
use crate::ui::{
    AddrBarText, AddressBarFocus, AddressEdit, BrowserActivityMarker, BrowserNavButton,
    BrowserPageWebview, BrowserToolbarState, HostActivityEntity, NavAction, PageWebviewOf,
};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, KeyCode};
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, JustifyContent, Val};
use bevy_cef::prelude::*;
use ozmux_configs::browser::resolve_omnibox_input;
use ozmux_multiplexer::{ActivityKind, AttachedSession, MultiplexerCommands, SessionMarker};

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

/// Renders each address-bar `Text` from its host's edit buffer (when that host
/// owns address-bar focus) or its toolbar-state URL (when unfocused).
fn render_address_text(
    focus: Res<AddressBarFocus>,
    hosts: Query<(Entity, &BrowserToolbarState, &AddressEdit), With<BrowserActivityMarker>>,
    children: Query<&Children>,
    mut texts: Query<&mut Text, With<AddrBarText>>,
) {
    for (host, state, edit) in hosts.iter() {
        let display = if focus.0 == Some(host) {
            edit.buffer.clone()
        } else {
            state.url.clone()
        };
        for descendant in descendants(host, &children) {
            if let Ok(mut text) = texts.get_mut(descendant) {
                if text.0 != display {
                    text.0 = display.clone();
                }
            }
        }
    }
}

/// Routes toolbar button presses to `bevy_cef` navigation requests.
fn drive_nav_buttons(
    mut commands: Commands,
    buttons: Query<(&Interaction, &BrowserNavButton), Changed<Interaction>>,
    pages: Query<&BrowserPageWebview>,
) {
    for (interaction, button) in buttons.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Ok(page) = pages.get(button.host) else {
            continue;
        };
        let webview = page.0;
        match button.action {
            NavAction::Back => commands.trigger(RequestGoBack { webview }),
            NavAction::Forward => commands.trigger(RequestGoForward { webview }),
            NavAction::Reload => commands.trigger(RequestReload { webview }),
        }
    }
}

/// Depth-first descendants of `root` (excluding `root`).
fn descendants(root: Entity, children: &Query<&Children>) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack: Vec<Entity> = children
        .get(root)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    while let Some(e) = stack.pop() {
        out.push(e);
        if let Ok(c) = children.get(e) {
            stack.extend(c.iter());
        }
    }
    out
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

/// Applies keyboard input to the focused browser host's address-bar buffer.
/// Enter resolves the omnibox and navigates the page webview, then blurs; Esc
/// blurs without navigating.
fn browser_address_editor(
    mut commands: Commands,
    mut focus: ResMut<AddressBarFocus>,
    mut events: MessageReader<KeyboardInput>,
    configs: Res<OzmuxConfigsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    mut clipboard: Option<ResMut<Clipboard>>,
    mut hosts: Query<(&mut AddressEdit, &BrowserPageWebview)>,
) {
    let Some(host) = focus.0 else {
        events.clear();
        return;
    };
    let Ok((mut edit, page)) = hosts.get_mut(host) else {
        focus.0 = None;
        events.clear();
        return;
    };
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        let cmd = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
        if cmd && matches!(&ev.logical_key, Key::Character(s) if s.eq_ignore_ascii_case("v")) {
            if let Some(clip) = clipboard.as_mut() {
                if let Some(text) = clip.read() {
                    let one_line: String = text.split(['\n', '\r']).collect();
                    insert_str(&mut edit, &one_line);
                }
            }
            continue;
        }
        match &ev.logical_key {
            Key::Enter => {
                let url = resolve_omnibox_input(&edit.buffer, &configs.browser.search_template);
                if !url.is_empty() {
                    commands.trigger(RequestNavigate { webview: page.0, url });
                }
                focus.0 = None;
                return;
            }
            Key::Escape => {
                focus.0 = None;
                return;
            }
            Key::Backspace => backspace(&mut edit),
            Key::Delete => delete(&mut edit),
            Key::ArrowLeft => caret_left(&mut edit),
            Key::ArrowRight => caret_right(&mut edit),
            Key::Home => caret_home(&mut edit),
            Key::End => caret_end(&mut edit),
            Key::Space => insert_char(&mut edit, ' '),
            Key::Character(s) => {
                for c in s.chars() {
                    insert_char(&mut edit, c);
                }
            }
            _ => {}
        }
    }
}

/// `Cmd+L` focuses the active browser pane's address bar (browser convention).
fn focus_address_bar_on_cmd_l(
    keys: Res<ButtonInput<KeyCode>>,
    mut focus: ResMut<AddressBarFocus>,
    mux: MultiplexerCommands,
    attached: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>,
    registry: Res<crate::ui::registry::ActivityEntityRegistry>,
    browser_hosts: Query<(), With<BrowserActivityMarker>>,
    mut edits: Query<(&mut AddressEdit, &BrowserToolbarState)>,
) {
    let cmd = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    if !(cmd && keys.just_pressed(KeyCode::KeyL)) {
        return;
    }
    let Some(session) = attached.iter().next() else {
        return;
    };
    let Some(pane) = mux.sessions_active_pane(session) else {
        return;
    };
    let Some(activity) = mux.panes_active_activity(pane) else {
        return;
    };
    let Some(host) = registry.get(activity) else {
        return;
    };
    if !browser_hosts.contains(host) {
        return;
    }
    let Ok((mut edit, state)) = edits.get_mut(host) else {
        return;
    };
    edit.buffer = state.url.clone();
    edit.caret = edit.buffer.chars().count();
    focus.0 = Some(host);
}

/// Returns the byte offset in `e.buffer` for the character at `idx`.
fn char_byte(e: &AddressEdit, idx: usize) -> usize {
    e.buffer
        .char_indices()
        .nth(idx)
        .map(|(b, _)| b)
        .unwrap_or(e.buffer.len())
}

/// Returns the number of Unicode scalar values in `e.buffer`.
fn char_count(e: &AddressEdit) -> usize {
    e.buffer.chars().count()
}

/// Inserts `c` at the caret position and advances the caret by one.
fn insert_char(e: &mut AddressEdit, c: char) {
    let at = char_byte(e, e.caret);
    e.buffer.insert(at, c);
    e.caret += 1;
}

/// Inserts `s` at the caret position and advances the caret by `s.chars().count()`.
fn insert_str(e: &mut AddressEdit, s: &str) {
    let at = char_byte(e, e.caret);
    e.buffer.insert_str(at, s);
    e.caret += s.chars().count();
}

/// Removes the character immediately before the caret and moves the caret left by one.
fn backspace(e: &mut AddressEdit) {
    if e.caret == 0 {
        return;
    }
    let start = char_byte(e, e.caret - 1);
    let end = char_byte(e, e.caret);
    e.buffer.replace_range(start..end, "");
    e.caret -= 1;
}

/// Removes the character immediately at (after) the caret; the caret does not move.
fn delete(e: &mut AddressEdit) {
    if e.caret >= char_count(e) {
        return;
    }
    let start = char_byte(e, e.caret);
    let end = char_byte(e, e.caret + 1);
    e.buffer.replace_range(start..end, "");
}

/// Moves the caret one character to the left, clamped at 0.
fn caret_left(e: &mut AddressEdit) {
    e.caret = e.caret.saturating_sub(1);
}

/// Moves the caret one character to the right, clamped at `char_count`.
fn caret_right(e: &mut AddressEdit) {
    e.caret = (e.caret + 1).min(char_count(e));
}

/// Moves the caret to the start of the buffer.
fn caret_home(e: &mut AddressEdit) {
    e.caret = 0;
}

/// Moves the caret to the end of the buffer.
fn caret_end(e: &mut AddressEdit) {
    e.caret = char_count(e);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyboardInput, NativeKeyCode};
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

    #[derive(Resource, Default)]
    struct Captured(Vec<Entity>);

    #[test]
    fn back_button_press_triggers_request_go_back() {
        let mut app = make_test_app();
        app.init_resource::<Captured>();
        app.add_systems(Update, (build_browser_chrome, drive_nav_buttons).chain());
        app.add_observer(|ev: On<RequestGoBack>, mut r: ResMut<Captured>| {
            r.0.push(ev.webview);
        });

        let host = app
            .world_mut()
            .spawn((BrowserActivityMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update(); // build chrome

        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;

        // Find the Back button entity.
        let back_btn: Entity = {
            let mut q = app.world_mut().query::<(Entity, &BrowserNavButton)>();
            q.iter(app.world())
                .find(|(_, b)| b.action == NavAction::Back && b.host == host)
                .map(|(e, _)| e)
                .expect("back button must exist")
        };

        // Simulate a press by inserting Interaction::Pressed.
        app.world_mut().entity_mut(back_btn).insert(Interaction::Pressed);
        app.update();

        let captured = app.world().resource::<Captured>();
        assert_eq!(captured.0, vec![page], "RequestGoBack must fire with the page webview");
    }

    #[test]
    fn address_text_follows_toolbar_state_when_unfocused() {
        let mut app = make_test_app();
        app.init_resource::<crate::ui::AddressBarFocus>();
        app.add_systems(Update, (build_browser_chrome, render_address_text).chain());
        let host = app
            .world_mut()
            .spawn((BrowserActivityMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update(); // build chrome + render (empty url)
        app.world_mut().get_mut::<BrowserToolbarState>(host).unwrap().url =
            "https://example.com".into();
        app.update(); // render picks up the new url

        let mut found: Option<String> = None;
        let mut q = app
            .world_mut()
            .query_filtered::<&Text, With<crate::ui::AddrBarText>>();
        for text in q.iter(app.world()) {
            found = Some(text.0.clone());
        }
        assert_eq!(found.as_deref(), Some("https://example.com"));
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

    use crate::ui::AddressEdit as AE;

    fn edit(s: &str, caret: usize) -> AE {
        AE { buffer: s.into(), caret }
    }

    #[test]
    fn address_edit_insert_at_caret() {
        let mut e = edit("ac", 1);
        super::insert_char(&mut e, 'b');
        assert_eq!(e.buffer, "abc");
        assert_eq!(e.caret, 2);
    }

    #[test]
    fn address_edit_backspace_and_delete() {
        let mut e = edit("abc", 2);
        super::backspace(&mut e);
        assert_eq!((e.buffer.as_str(), e.caret), ("ac", 1));
        super::delete(&mut e);
        assert_eq!((e.buffer.as_str(), e.caret), ("a", 1));
    }

    #[test]
    fn address_edit_caret_motion_clamps() {
        let mut e = edit("abc", 0);
        super::caret_left(&mut e);
        assert_eq!(e.caret, 0);
        super::caret_right(&mut e);
        assert_eq!(e.caret, 1);
        super::caret_end(&mut e);
        assert_eq!(e.caret, 3);
        super::caret_home(&mut e);
        assert_eq!(e.caret, 0);
    }

    #[test]
    fn address_edit_insert_str_paste() {
        let mut e = edit("ab", 1);
        super::insert_str(&mut e, "XY");
        assert_eq!((e.buffer.as_str(), e.caret), ("aXYb", 3));
    }

    #[test]
    fn address_edit_utf8_safe() {
        let mut e = edit("aあc", 2); // caret between あ and c
        super::insert_char(&mut e, 'b');
        assert_eq!(e.buffer, "aあbc");
        assert_eq!(e.caret, 3);
        super::backspace(&mut e); // removes 'b'
        assert_eq!(e.buffer, "aあc");
    }

    fn key_press(window: Entity, logical: Key) -> KeyboardInput {
        KeyboardInput {
            key_code: bevy::input::keyboard::KeyCode::Unidentified(NativeKeyCode::Unidentified),
            logical_key: logical,
            state: ButtonState::Pressed,
            repeat: false,
            window,
            text: None,
        }
    }

    #[derive(bevy::ecs::resource::Resource, Default)]
    struct Navigated(Vec<(Entity, String)>);

    #[test]
    fn enter_resolves_and_navigates_then_clears_focus() {
        let mut app = make_test_app();
        app.init_resource::<crate::ui::AddressBarFocus>();
        app.insert_resource(bevy::input::ButtonInput::<bevy::input::keyboard::KeyCode>::default());
        app.add_message::<KeyboardInput>();
        app.add_systems(Update, build_browser_chrome);
        app.add_systems(Update, browser_address_editor.after(build_browser_chrome));

        let window = app.world_mut().spawn_empty().id();
        let host = app
            .world_mut()
            .spawn((BrowserActivityMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update();
        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;

        app.world_mut().resource_mut::<crate::ui::AddressBarFocus>().0 = Some(host);
        for c in "github.com".chars() {
            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>()
                .write(key_press(window, Key::Character(c.to_string().into())));
        }
        app.update();
        assert_eq!(app.world().get::<AddressEdit>(host).unwrap().buffer, "github.com");

        app.init_resource::<Navigated>();
        app.add_observer(|ev: On<RequestNavigate>, mut n: ResMut<Navigated>| {
            n.0.push((ev.webview, ev.url.clone()));
        });

        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>()
            .write(key_press(window, Key::Enter));
        app.update();
        app.world_mut().flush();

        let nav = app.world().resource::<Navigated>();
        assert_eq!(nav.0, vec![(page, "https://github.com".to_string())]);
        assert_eq!(
            app.world().resource::<crate::ui::AddressBarFocus>().0,
            None,
            "Enter clears focus"
        );
    }
}
