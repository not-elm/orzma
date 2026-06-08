//! Browser surface rendering: a native back/forward/reload + address-bar
//! toolbar over a `bevy_cef` page webview. The surface host (a column) gets
//! two persistent children built once — a toolbar and a
//! page-webview node — and (in a later phase) a CEF webview attached to the
//! laid-out page child after host-side omnibox resolution.

use crate::clipboard::Clipboard;
use crate::configs::OzmuxConfigsResource;
use crate::system_set::OzmuxSystems;
use crate::ui::palette;
use crate::ui::{
    AddrBarText, AddressBarFocus, AddressEdit, BrowserNavButton, BrowserPageWebview,
    BrowserSurfaceMarker, BrowserToolbarState, NavAction, PageWebviewOf,
};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, JustifyContent, Val};
use bevy::window::Ime;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use bevy_cef::prelude::*;
use ozmux_configs::browser::resolve_omnibox_input;
use ozmux_multiplexer::SurfaceKind;
use ozmux_multiplexer::{AttachedWorkspace, MultiplexerQuery, WorkspaceMarker};

const TOOLBAR_HEIGHT_PX: f32 = 32.0;
/// Vimium-style scroll keybindings, injected into each browser page webview as
/// a `PreloadScripts` entry. Self-contained IIFE; see `browser_render/vim_scroll.js`.
const VIM_SCROLL_JS: &str = include_str!("browser_render/vim_scroll.js");

/// Wires the browser surface renderer: two-phase mount, toolbar render +
/// navigation, address-bar editor + focus, and navigation-state observers.
pub(crate) struct OzmuxBrowserRenderPlugin;

impl Plugin for OzmuxBrowserRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<crate::ui::AddressBarFocus>()
            .add_observer(on_address_changed)
            .add_observer(on_loading_state_changed)
            .add_systems(
                Update,
                (
                    build_browser_chrome.in_set(OzmuxSystems::SetupSurface),
                    attach_browser_webview.in_set(OzmuxSystems::SetupSurface),
                    render_address_text,
                    drive_nav_buttons,
                    sync_nav_button_enabled,
                    nav_button_hover_cursor.after(crate::input::InputPhase::Hover),
                ),
            );
        app.add_systems(
            Update,
            (
                focus_address_bar_on_click,
                apply_ime_to_address_bar,
                focus_address_bar_on_cmd_l.before(crate::input::dispatch_focused_key),
                blur_address_bar_on_focus_leave
                    .after(crate::input::InputPhase::Dispatch)
                    .before(crate::input::dispatch_focused_key)
                    .run_if(address_bar_is_focused),
                browser_address_editor.after(crate::input::dispatch_focused_key),
            ),
        );
    }
}

/// Builds the toolbar + empty page-webview children for each laid-out browser
/// host that has not been built yet (no `BrowserPageWebview` pointer).
fn build_browser_chrome(
    mut commands: Commands,
    hosts: Query<
        (Entity, &ComputedNode),
        (With<BrowserSurfaceMarker>, Without<BrowserPageWebview>),
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
            .spawn((
                Button,
                Text::new(""),
                AddrBarText(host),
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
            ))
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
        commands
            .entity(toolbar)
            .add_children(&[back, forward, reload, addr]);

        let page = commands
            .spawn((
                Node {
                    flex_grow: 1.0,
                    width: Val::Percent(100.0),
                    ..default()
                },
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
    kinds: Query<&SurfaceKind>,
    mut states: Query<&mut BrowserToolbarState>,
) {
    for (page, computed, owner) in pages.iter() {
        let size = computed.size() * computed.inverse_scale_factor();
        if size.x < 1.0 || size.y < 1.0 {
            continue;
        }
        let Ok(SurfaceKind::Browser { initial_url, .. }) = kinds.get(owner.0) else {
            continue;
        };
        let raw = initial_url.as_deref().unwrap_or("");
        let resolved = resolve_omnibox_input(raw, &configs.browser.search_template);
        if resolved.is_empty() {
            continue;
        }
        // Seed the toolbar URL so the bar isn't blank (or caret-only on early
        // focus) until CEF fires its first AddressChanged.
        if let Ok(mut state) = states.get_mut(owner.0)
            && state.url.is_empty()
        {
            state.url = resolved.clone();
        }
        commands.entity(page).insert((
            WebviewSource::new(resolved),
            WebviewSize(size),
            PreloadScripts::from([VIM_SCROLL_JS.to_string()]),
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
    hosts: Query<(&BrowserToolbarState, &AddressEdit)>,
    mut texts: Query<(&AddrBarText, &mut Text)>,
) {
    for (addr, mut text) in texts.iter_mut() {
        let host = addr.0;
        let Ok((state, edit)) = hosts.get(host) else {
            continue;
        };
        let display = if focus.0 == Some(host) {
            render_with_caret(&edit.buffer, edit.caret)
        } else {
            state.url.clone()
        };
        if text.0 != display {
            text.0 = display;
        }
    }
}

/// Renders the edit buffer with a `|` caret inserted at the char index `caret`.
fn render_with_caret(buffer: &str, caret: usize) -> String {
    let chars: Vec<char> = buffer.chars().collect();
    let at = caret.min(chars.len());
    let mut s: String = chars[..at].iter().collect();
    s.push('|');
    s.extend(chars[at..].iter());
    s
}

/// Routes toolbar button presses to `bevy_cef` navigation requests, skipping
/// Back/Forward when the host state indicates they are unavailable.
fn drive_nav_buttons(
    mut commands: Commands,
    buttons: Query<(&Interaction, &BrowserNavButton), Changed<Interaction>>,
    pages: Query<&BrowserPageWebview>,
    states: Query<&BrowserToolbarState>,
) {
    for (interaction, button) in buttons.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let enabled = match button.action {
            NavAction::Back => states
                .get(button.host)
                .map(|s| s.can_go_back)
                .unwrap_or(false),
            NavAction::Forward => states
                .get(button.host)
                .map(|s| s.can_go_forward)
                .unwrap_or(false),
            NavAction::Reload => true,
        };
        if !enabled {
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

/// Distinguishes enabled vs disabled back/forward buttons: an enabled button
/// shows a bright `FOREGROUND` glyph on a lighter `TAB_ACTIVE_BG` background; a
/// disabled one is dimmed to `MUTED` on a transparent background. Mirrors the
/// tab bar's active/inactive treatment so the states read consistently.
fn sync_nav_button_enabled(
    states: Query<&BrowserToolbarState>,
    mut buttons: Query<(&BrowserNavButton, &mut BackgroundColor, &Children)>,
    mut text_colors: Query<&mut TextColor>,
) {
    for (button, mut bg, children) in buttons.iter_mut() {
        let enabled = match button.action {
            NavAction::Back => states
                .get(button.host)
                .map(|s| s.can_go_back)
                .unwrap_or(false),
            NavAction::Forward => states
                .get(button.host)
                .map(|s| s.can_go_forward)
                .unwrap_or(false),
            NavAction::Reload => true,
        };
        let (background, glyph) = if enabled {
            (palette::TAB_ACTIVE_BG, palette::FOREGROUND)
        } else {
            (Color::NONE, palette::MUTED)
        };
        if bg.0 != background {
            bg.0 = background;
        }
        for child in children.iter() {
            if let Ok(mut tc) = text_colors.get_mut(child)
                && tc.0 != glyph
            {
                tc.0 = glyph;
            }
        }
    }
}

/// Shows a pointer cursor while the mouse hovers any nav button, so the
/// back/forward/reload buttons read as clickable. Runs after `InputPhase::Hover`
/// so it wins over the hyperlink system's baseline cursor write. Leaving a
/// button onto the native toolbar reverts to the arrow when the hyperlink
/// system re-asserts; moving onto the CEF page hands the cursor to `bevy_cef`,
/// which re-asserts the page cursor on the next pointer event.
fn nav_button_hover_cursor(
    buttons: Query<&Interaction, With<BrowserNavButton>>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
) {
    let hovering = buttons
        .iter()
        .any(|i| matches!(i, Interaction::Hovered | Interaction::Pressed));
    if !hovering {
        return;
    }
    let Ok(mut icon) = cursor_icons.single_mut() else {
        return;
    };
    if !matches!(&*icon, CursorIcon::System(e) if *e == SystemCursorIcon::Pointer) {
        *icon = CursorIcon::System(SystemCursorIcon::Pointer);
    }
}

fn spawn_nav_button(
    commands: &mut Commands,
    host: Entity,
    action: NavAction,
    label: &str,
) -> Entity {
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
            BackgroundColor(Color::NONE),
            BrowserNavButton { host, action },
        ))
        .with_children(|p| {
            p.spawn((Text::new(label.to_string()), TextColor(palette::MUTED)));
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
    ime_state: Res<crate::input::ime::ImeState>,
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
        // NOTE: while an IME composition is active the OS owns the keystrokes;
        // the committed text arrives via `apply_ime_to_address_bar`. Skipping
        // raw keys here prevents inserting pre-composition ASCII on platforms
        // that still deliver `KeyboardInput` during composition.
        if ime_state.is_composing() {
            continue;
        }
        let cmd = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
        let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
        // NOTE: strict Cmd+V paste (lowercase, no other modifiers). The terminal
        // path now resolves paste through the rebindable `paste` binding
        // (Action::Paste, default Cmd+V); this omnibox check stays hardcoded to the
        // default chord, so a user who rebinds `paste` will not see the new chord
        // here. Accepted divergence — unifying the omnibox with the binding table is
        // a tracked follow-up.
        if cmd
            && !ctrl
            && !shift
            && !alt
            && matches!(&ev.logical_key, Key::Character(s) if s.as_str() == "v")
        {
            if let Some(clip) = clipboard.as_mut()
                && let Some(text) = clip.read()
            {
                let one_line: String = text.split(['\n', '\r']).collect();
                insert_str(&mut edit, &one_line);
            }
            continue;
        }
        if cmd || ctrl {
            continue;
        }
        match &ev.logical_key {
            Key::Enter => {
                let url = resolve_omnibox_input(&edit.buffer, &configs.browser.search_template);
                if !url.is_empty() {
                    commands.trigger(RequestNavigate {
                        webview: page.0,
                        url,
                    });
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

/// Routes IME-committed text (`Ime::Commit`) into the focused address bar's edit
/// buffer. The OS candidate window shows the preedit (anchored by
/// `ime_policy_system`); on commit the text is inserted here. CJK input into the
/// URL/search bar would otherwise be lost, since `read_ime_events` forwards
/// commits to the active terminal, which a browser pane has none of.
fn apply_ime_to_address_bar(
    mut events: MessageReader<Ime>,
    focus: Res<AddressBarFocus>,
    mut edits: Query<&mut AddressEdit>,
) {
    let Some(host) = focus.0 else {
        events.clear();
        return;
    };
    let Ok(mut edit) = edits.get_mut(host) else {
        events.clear();
        return;
    };
    for ev in events.read() {
        if let Ime::Commit { value, .. } = ev {
            insert_str(&mut edit, value);
        }
    }
}

/// Clicking the address bar focuses it (seeding the buffer from the current URL).
fn focus_address_bar_on_click(
    mut focus: ResMut<AddressBarFocus>,
    clicked: Query<(&Interaction, &AddrBarText), Changed<Interaction>>,
    mut edits: Query<(&mut AddressEdit, &BrowserToolbarState)>,
) {
    for (interaction, addr) in clicked.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let host = addr.0;
        let Ok((mut edit, state)) = edits.get_mut(host) else {
            continue;
        };
        edit.buffer = state.url.clone();
        edit.caret = edit.buffer.chars().count();
        focus.0 = Some(host);
    }
}

/// `Cmd+L` focuses the active browser pane's address bar (browser convention).
fn focus_address_bar_on_cmd_l(
    keys: Res<ButtonInput<KeyCode>>,
    mut focus: ResMut<AddressBarFocus>,
    mux: MultiplexerQuery,
    attached: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    browser_surfaces: Query<(), With<BrowserSurfaceMarker>>,
    mut edits: Query<(&mut AddressEdit, &BrowserToolbarState)>,
) {
    let cmd = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    if !(cmd && keys.just_pressed(KeyCode::KeyL)) {
        return;
    }
    let Some(workspace) = attached.iter().next() else {
        return;
    };
    let Some(pane) = mux.workspaces_active_pane(workspace) else {
        return;
    };
    let Some(surface) = mux.panes_active_surface(pane) else {
        return;
    };
    if !browser_surfaces.contains(surface) {
        return;
    }
    let Ok((mut edit, state)) = edits.get_mut(surface) else {
        return;
    };
    edit.buffer = state.url.clone();
    edit.caret = edit.buffer.chars().count();
    focus.0 = Some(surface);
}

/// Run condition gating `blur_address_bar_on_focus_leave`: only poll while the
/// address bar actually owns focus, so its heavy params (`MultiplexerQuery`,
/// queries) are not fetched on the common unfocused path.
fn address_bar_is_focused(focus: Res<AddressBarFocus>) -> bool {
    focus.0.is_some()
}

/// Clears `AddressBarFocus` when focus leaves the bar — when the active pane is
/// no longer the focused browser host, or a left-click lands outside any address
/// bar. Without this the bar stays focused after the user moves to a terminal,
/// and `dispatch_focused_key`'s guard then suppresses all keyboard input.
fn blur_address_bar_on_focus_leave(
    mut focus: ResMut<AddressBarFocus>,
    mux: MultiplexerQuery,
    attached: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    mouse: Res<ButtonInput<MouseButton>>,
    addr_bars: Query<&Interaction, With<AddrBarText>>,
) {
    let Some(focused_surface) = focus.0 else {
        return;
    };
    let active_surface = attached
        .iter()
        .next()
        .and_then(|ws| mux.workspaces_active_pane(ws))
        .and_then(|p| mux.panes_active_surface(p));
    let pane_left = active_surface != Some(focused_surface);
    let clicked_outside = mouse.just_pressed(MouseButton::Left)
        && !addr_bars.iter().any(|i| *i == Interaction::Pressed);
    if pane_left || clicked_outside {
        focus.0 = None;
    }
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

#[cfg(all(test, not(feature = "thin-client")))]
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
            .insert_resource(crate::configs::OzmuxConfigsResource(
                ozmux_configs::OzmuxConfigs::default(),
            ));
        app
    }

    fn laid_out_node(size: Vec2) -> ComputedNode {
        ComputedNode {
            size,
            inverse_scale_factor: 1.0,
            ..ComputedNode::DEFAULT
        }
    }

    #[test]
    fn build_chrome_spawns_toolbar_and_empty_page_child() {
        let mut app = make_test_app();
        app.add_systems(Update, build_browser_chrome);
        let host = app
            .world_mut()
            .spawn((BrowserSurfaceMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update();

        let page = app
            .world()
            .get::<BrowserPageWebview>(host)
            .expect("host gets BrowserPageWebview")
            .0;
        assert!(
            app.world().get::<WebviewSource>(page).is_none(),
            "page child must be an empty Node (no webview yet)"
        );
        assert_eq!(
            app.world().get::<PageWebviewOf>(page).map(|p| p.0),
            Some(host),
            "page child points back to host"
        );
        assert!(app.world().get::<BrowserToolbarState>(host).is_some());
        assert!(app.world().get::<AddressEdit>(host).is_some());
    }

    #[test]
    fn build_chrome_is_idempotent() {
        let mut app = make_test_app();
        app.add_systems(Update, build_browser_chrome);
        let host = app
            .world_mut()
            .spawn((BrowserSurfaceMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update();
        let first = app.world().get::<BrowserPageWebview>(host).unwrap().0;
        app.update();
        let second = app.world().get::<BrowserPageWebview>(host).unwrap().0;
        assert_eq!(first, second, "chrome built exactly once");
    }

    #[test]
    fn attach_resolves_omnibox_and_seeds_child_size() {
        use ozmux_multiplexer::SurfaceKind;
        let mut app = make_test_app();
        app.add_systems(
            Update,
            (build_browser_chrome, attach_browser_webview).chain(),
        );

        // The Surface entity IS its own host: it carries the SurfaceKind, the
        // BrowserSurfaceMarker, and the laid-out node.
        let host = app
            .world_mut()
            .spawn((
                SurfaceKind::Browser {
                    initial_url: Some("github.com".into()),
                    profile: Default::default(),
                },
                BrowserSurfaceMarker,
                laid_out_node(Vec2::new(800.0, 600.0)),
            ))
            .id();
        // NOTE: first tick builds chrome; attach is a no-op until the page child is laid out.
        app.update();

        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;
        app.world_mut()
            .entity_mut(page)
            .insert(laid_out_node(Vec2::new(800.0, 568.0)));
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
            .spawn((BrowserSurfaceMarker, laid_out_node(Vec2::new(800.0, 600.0))))
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
            .spawn((BrowserSurfaceMarker, laid_out_node(Vec2::new(800.0, 600.0))))
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
            .spawn((BrowserSurfaceMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update(); // build chrome

        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;

        // Enable back navigation so drive_nav_buttons permits the press.
        app.world_mut()
            .get_mut::<BrowserToolbarState>(host)
            .unwrap()
            .can_go_back = true;

        // Find the Back button entity.
        let back_btn: Entity = {
            let mut q = app.world_mut().query::<(Entity, &BrowserNavButton)>();
            q.iter(app.world())
                .find(|(_, b)| b.action == NavAction::Back && b.host == host)
                .map(|(e, _)| e)
                .expect("back button must exist")
        };

        // Simulate a press by inserting Interaction::Pressed.
        app.world_mut()
            .entity_mut(back_btn)
            .insert(Interaction::Pressed);
        app.update();

        let captured = app.world().resource::<Captured>();
        assert_eq!(
            captured.0,
            vec![page],
            "RequestGoBack must fire with the page webview"
        );
    }

    #[test]
    fn nav_button_glyph_dims_when_disabled() {
        let mut app = make_test_app();
        app.add_systems(
            Update,
            (build_browser_chrome, sync_nav_button_enabled).chain(),
        );
        let host = app
            .world_mut()
            .spawn((BrowserSurfaceMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update(); // build chrome
        {
            let mut state = app
                .world_mut()
                .get_mut::<BrowserToolbarState>(host)
                .unwrap();
            state.can_go_back = true;
            state.can_go_forward = false;
        }
        app.update(); // sync drives the button colors

        let glyph = |app: &mut App, action: NavAction| -> Color {
            let btn = {
                let mut q = app.world_mut().query::<(Entity, &BrowserNavButton)>();
                q.iter(app.world())
                    .find(|(_, b)| b.action == action && b.host == host)
                    .map(|(e, _)| e)
                    .expect("button must exist")
            };
            let children: Vec<Entity> = app
                .world()
                .get::<Children>(btn)
                .expect("button has a label child")
                .iter()
                .collect();
            children
                .into_iter()
                .find_map(|c| app.world().get::<TextColor>(c).map(|tc| tc.0))
                .expect("label has a TextColor")
        };

        assert_eq!(
            glyph(&mut app, NavAction::Back),
            palette::FOREGROUND,
            "enabled back glyph is bright (FOREGROUND)"
        );
        assert_eq!(
            glyph(&mut app, NavAction::Forward),
            palette::MUTED,
            "disabled forward glyph is muted (MUTED)"
        );
    }

    #[test]
    fn nav_button_hover_sets_pointer_cursor() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let window = world
            .spawn((PrimaryWindow, CursorIcon::System(SystemCursorIcon::Text)))
            .id();
        let host = world.spawn_empty().id();
        let button = world
            .spawn((
                BrowserNavButton {
                    host,
                    action: NavAction::Back,
                },
                Interaction::Hovered,
            ))
            .id();

        world.run_system_once(nav_button_hover_cursor).unwrap();
        assert!(
            matches!(
                world.get::<CursorIcon>(window),
                Some(CursorIcon::System(SystemCursorIcon::Pointer))
            ),
            "hovering a nav button sets the pointer cursor"
        );

        // Not hovering: the system no-ops, leaving the cursor as the hover phase set it.
        *world.get_mut::<Interaction>(button).unwrap() = Interaction::None;
        *world.get_mut::<CursorIcon>(window).unwrap() = CursorIcon::System(SystemCursorIcon::Text);
        world.run_system_once(nav_button_hover_cursor).unwrap();
        assert!(
            matches!(
                world.get::<CursorIcon>(window),
                Some(CursorIcon::System(SystemCursorIcon::Text))
            ),
            "no hovered button leaves the cursor unchanged"
        );
    }

    /// Builds an app with one attached workspace whose active surface is its
    /// own host, and the address bar focused on that surface — so
    /// `blur_address_bar_on_focus_leave`'s pane-left condition is false by default.
    fn focused_app_on_active_host() -> (App, Entity) {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, MultiplexerPlugin};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.init_resource::<crate::ui::AddressBarFocus>();
        app.init_resource::<ButtonInput<MouseButton>>();

        let (workspace, _pane, surface) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.workspace, o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().flush();
        app.world_mut()
            .entity_mut(workspace)
            .insert(AttachedWorkspace);

        app.world_mut()
            .resource_mut::<crate::ui::AddressBarFocus>()
            .0 = Some(surface);
        (app, surface)
    }

    #[test]
    fn blur_clears_focus_when_active_pane_is_not_the_focused_host() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, _active_host) = focused_app_on_active_host();
        // Focus a DIFFERENT host than the active pane's host.
        let other_host = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<crate::ui::AddressBarFocus>()
            .0 = Some(other_host);

        app.world_mut()
            .run_system_once(blur_address_bar_on_focus_leave)
            .unwrap();

        assert_eq!(
            app.world().resource::<crate::ui::AddressBarFocus>().0,
            None,
            "the bar blurs when the active pane is not the focused host"
        );
    }

    #[test]
    fn blur_clears_focus_on_left_click_outside_bar() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, _host) = focused_app_on_active_host();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);

        app.world_mut()
            .run_system_once(blur_address_bar_on_focus_leave)
            .unwrap();

        assert_eq!(
            app.world().resource::<crate::ui::AddressBarFocus>().0,
            None,
            "a left-click outside any address bar blurs the bar"
        );
    }

    #[test]
    fn blur_keeps_focus_when_clicking_the_bar() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, host) = focused_app_on_active_host();
        app.world_mut()
            .spawn((crate::ui::AddrBarText(host), Interaction::Pressed));
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);

        app.world_mut()
            .run_system_once(blur_address_bar_on_focus_leave)
            .unwrap();

        assert_eq!(
            app.world().resource::<crate::ui::AddressBarFocus>().0,
            Some(host),
            "clicking the address bar itself does not blur it"
        );
    }

    #[test]
    fn blur_keeps_focus_when_staying_on_focused_pane() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, host) = focused_app_on_active_host();

        app.world_mut()
            .run_system_once(blur_address_bar_on_focus_leave)
            .unwrap();

        assert_eq!(
            app.world().resource::<crate::ui::AddressBarFocus>().0,
            Some(host),
            "staying on the focused browser pane keeps the bar focused"
        );
    }

    #[test]
    fn ime_commit_inserts_into_focused_address_bar() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.init_resource::<crate::ui::AddressBarFocus>();
        app.add_message::<Ime>();
        let host = app.world_mut().spawn(AddressEdit::default()).id();
        app.world_mut()
            .resource_mut::<crate::ui::AddressBarFocus>()
            .0 = Some(host);
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<Ime>>()
            .write(Ime::Commit {
                window: Entity::PLACEHOLDER,
                value: "こんにちは".into(),
            });

        app.world_mut()
            .run_system_once(apply_ime_to_address_bar)
            .unwrap();

        assert_eq!(
            app.world().get::<AddressEdit>(host).unwrap().buffer,
            "こんにちは",
            "Ime::Commit must be inserted into the focused address bar's buffer"
        );
    }

    #[test]
    fn address_text_follows_toolbar_state_when_unfocused() {
        let mut app = make_test_app();
        app.init_resource::<crate::ui::AddressBarFocus>();
        app.add_systems(Update, (build_browser_chrome, render_address_text).chain());
        let host = app
            .world_mut()
            .spawn((BrowserSurfaceMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update(); // build chrome + render (empty url)
        app.world_mut()
            .get_mut::<BrowserToolbarState>(host)
            .unwrap()
            .url = "https://example.com".into();
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
    fn attach_injects_vim_scroll_preload() {
        use ozmux_multiplexer::SurfaceKind;
        let mut app = make_test_app();
        app.add_systems(
            Update,
            (build_browser_chrome, attach_browser_webview).chain(),
        );

        let host = app
            .world_mut()
            .spawn((
                SurfaceKind::Browser {
                    initial_url: Some("github.com".into()),
                    profile: Default::default(),
                },
                BrowserSurfaceMarker,
                laid_out_node(Vec2::new(800.0, 600.0)),
            ))
            .id();
        // NOTE: first tick builds chrome; attach is a no-op until the page child is laid out.
        app.update();

        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;
        app.world_mut()
            .entity_mut(page)
            .insert(laid_out_node(Vec2::new(800.0, 568.0)));
        // NOTE: page child now has a ComputedNode, so attach fires this tick.
        app.update();

        let preload = app
            .world()
            .get::<PreloadScripts>(page)
            .expect("page webview must carry the vim-scroll PreloadScript");
        assert!(
            preload
                .0
                .iter()
                .any(|s| s.contains("window.__ozmuxVimScroll")),
            "the vim-scroll content script must be injected into the browser page webview"
        );
    }

    #[test]
    fn attach_skips_empty_input() {
        use ozmux_multiplexer::SurfaceKind;
        let mut app = make_test_app();
        app.add_systems(
            Update,
            (build_browser_chrome, attach_browser_webview).chain(),
        );
        let host = app
            .world_mut()
            .spawn((
                SurfaceKind::Browser {
                    initial_url: None,
                    profile: Default::default(),
                },
                BrowserSurfaceMarker,
                laid_out_node(Vec2::new(800.0, 600.0)),
            ))
            .id();
        app.update();
        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;
        app.world_mut()
            .entity_mut(page)
            .insert(laid_out_node(Vec2::new(800.0, 568.0)));
        app.update();
        assert!(
            app.world().get::<WebviewSource>(page).is_none(),
            "empty initial_url resolves to empty; no webview attached"
        );
    }

    use crate::ui::AddressEdit as AE;

    fn edit(s: &str, caret: usize) -> AE {
        AE {
            buffer: s.into(),
            caret,
        }
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
    fn editor_ignores_text_when_cmd_held() {
        let mut app = make_test_app();
        app.init_resource::<crate::ui::AddressBarFocus>();
        app.init_resource::<crate::input::ime::ImeState>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.add_message::<KeyboardInput>();
        app.add_systems(Update, build_browser_chrome);
        app.add_systems(Update, browser_address_editor.after(build_browser_chrome));
        let window = app.world_mut().spawn_empty().id();
        let host = app
            .world_mut()
            .spawn((BrowserSurfaceMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update();
        app.world_mut()
            .resource_mut::<crate::ui::AddressBarFocus>()
            .0 = Some(host);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>()
            .write(key_press(window, Key::Character("l".into())));
        app.update();

        assert_eq!(
            app.world().get::<AddressEdit>(host).unwrap().buffer,
            "",
            "Cmd+<key> must not type into the address bar"
        );
    }

    #[test]
    fn enter_resolves_and_navigates_then_clears_focus() {
        let mut app = make_test_app();
        app.init_resource::<crate::ui::AddressBarFocus>();
        app.init_resource::<crate::input::ime::ImeState>();
        app.insert_resource(bevy::input::ButtonInput::<bevy::input::keyboard::KeyCode>::default());
        app.add_message::<KeyboardInput>();
        app.add_systems(Update, build_browser_chrome);
        app.add_systems(Update, browser_address_editor.after(build_browser_chrome));

        let window = app.world_mut().spawn_empty().id();
        let host = app
            .world_mut()
            .spawn((BrowserSurfaceMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update();
        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;

        app.world_mut()
            .resource_mut::<crate::ui::AddressBarFocus>()
            .0 = Some(host);
        for c in "github.com".chars() {
            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>()
                .write(key_press(window, Key::Character(c.to_string().into())));
        }
        app.update();
        assert_eq!(
            app.world().get::<AddressEdit>(host).unwrap().buffer,
            "github.com"
        );

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

    #[test]
    fn render_with_caret_inserts_pipe_at_position() {
        assert_eq!(super::render_with_caret("abc", 1), "a|bc");
        assert_eq!(super::render_with_caret("abc", 3), "abc|");
        assert_eq!(super::render_with_caret("", 0), "|");
    }

    #[test]
    fn click_address_bar_focuses_and_seeds_buffer() {
        let mut app = make_test_app();
        app.init_resource::<crate::ui::AddressBarFocus>();
        app.add_systems(
            Update,
            (build_browser_chrome, focus_address_bar_on_click).chain(),
        );

        let host = app
            .world_mut()
            .spawn((BrowserSurfaceMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update(); // build chrome

        app.world_mut()
            .get_mut::<BrowserToolbarState>(host)
            .unwrap()
            .url = "https://x.com".into();

        let addr_btn: Entity = {
            let mut q = app.world_mut().query::<(Entity, &AddrBarText)>();
            q.iter(app.world())
                .find(|(_, a)| a.0 == host)
                .map(|(e, _)| e)
                .expect("AddrBarText button must exist for host")
        };

        app.world_mut()
            .entity_mut(addr_btn)
            .insert(Interaction::Pressed);
        app.update();

        assert_eq!(
            app.world().resource::<crate::ui::AddressBarFocus>().0,
            Some(host),
            "clicking address bar must set focus to the host"
        );
        assert_eq!(
            app.world().get::<AddressEdit>(host).unwrap().buffer,
            "https://x.com",
            "buffer must be seeded from toolbar state URL"
        );
    }

    #[test]
    fn back_button_disabled_when_cannot_go_back() {
        let mut app = make_test_app();
        app.init_resource::<Captured>();
        app.add_systems(Update, (build_browser_chrome, drive_nav_buttons).chain());
        app.add_observer(|ev: On<RequestGoBack>, mut r: ResMut<Captured>| {
            r.0.push(ev.webview);
        });

        let host = app
            .world_mut()
            .spawn((BrowserSurfaceMarker, laid_out_node(Vec2::new(800.0, 600.0))))
            .id();
        app.update(); // build chrome

        // can_go_back is false by default; verify no event fires on press.
        let back_btn: Entity = {
            let mut q = app.world_mut().query::<(Entity, &BrowserNavButton)>();
            q.iter(app.world())
                .find(|(_, b)| b.action == NavAction::Back && b.host == host)
                .map(|(e, _)| e)
                .expect("back button must exist")
        };

        app.world_mut()
            .entity_mut(back_btn)
            .insert(Interaction::Pressed);
        app.update();

        assert_eq!(
            app.world().resource::<Captured>().0,
            vec![],
            "RequestGoBack must NOT fire when can_go_back is false"
        );

        // Now enable back navigation and verify the event fires.
        app.world_mut()
            .get_mut::<BrowserToolbarState>(host)
            .unwrap()
            .can_go_back = true;

        app.world_mut()
            .entity_mut(back_btn)
            .insert(Interaction::Pressed);
        app.update();

        let page = app.world().get::<BrowserPageWebview>(host).unwrap().0;
        assert_eq!(
            app.world().resource::<Captured>().0,
            vec![page],
            "RequestGoBack must fire when can_go_back is true"
        );
    }
}

#[cfg(test)]
mod thin_compatible_tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyboardInput, NativeKeyCode};
    use bevy::prelude::MinimalPlugins;

    fn key_press(window: Entity, logical: Key) -> KeyboardInput {
        KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Unidentified),
            logical_key: logical,
            state: ButtonState::Pressed,
            repeat: false,
            window,
            text: None,
        }
    }

    #[derive(Resource, Default)]
    struct Navigated(Vec<(Entity, String)>);

    /// `browser_address_editor` resolves the omnibox and fires `RequestNavigate`
    /// on Enter, then clears focus — with NO `MultiplexerPlugin` and NO daemon,
    /// so it runs identically in both the local and thin builds.
    #[test]
    fn editor_enter_navigates_in_both_configs() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<AddressBarFocus>();
        app.init_resource::<crate::input::ime::ImeState>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(crate::configs::OzmuxConfigsResource(
            ozmux_configs::OzmuxConfigs::default(),
        ));
        app.add_message::<KeyboardInput>();
        app.add_systems(Update, browser_address_editor);

        let window = app.world_mut().spawn_empty().id();
        let page = app.world_mut().spawn_empty().id();
        let host = app
            .world_mut()
            .spawn((AddressEdit::default(), BrowserPageWebview(page)))
            .id();
        app.world_mut().resource_mut::<AddressBarFocus>().0 = Some(host);

        for c in "github.com".chars() {
            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>()
                .write(key_press(window, Key::Character(c.to_string().into())));
        }
        app.update();
        assert_eq!(
            app.world().get::<AddressEdit>(host).unwrap().buffer,
            "github.com",
            "typed characters accumulate in the focused bar's buffer",
        );

        app.init_resource::<Navigated>();
        app.add_observer(|ev: On<RequestNavigate>, mut n: ResMut<Navigated>| {
            n.0.push((ev.webview, ev.url.clone()));
        });
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>()
            .write(key_press(window, Key::Enter));
        app.update();
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<Navigated>().0,
            vec![(page, "https://github.com".to_string())],
            "Enter resolves the omnibox and fires RequestNavigate to the page webview",
        );
        assert_eq!(
            app.world().resource::<AddressBarFocus>().0,
            None,
            "Enter clears address-bar focus",
        );
    }
}
