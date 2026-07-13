//! Permanent multiplexer UI subtree under `UiRoot`: the always-visible window
//! bar and the workspace container that hosts each window's pane containers.

use crate::ui::UiRoot;
use bevy::prelude::*;

// NOTE: `confirm_prompt` must be `pub(crate)`, not the repo's default private
// submodule, because `apply_type` (`src/input/shortcuts/apply.rs`) gates on
// `confirm_prompt::ConfirmState` to keep an answering y/n out of the PTY.
pub(crate) mod confirm_prompt;
// NOTE: `divider_handle` must be `pub(crate)`, not the repo's default private
// submodule, because the Dispatch-phase mouse consumers
// (`dispatch_mouse_buttons`, `focus_pane_on_click`, `route_webview_pointer`
// under `src/input/mouse`) gate on `divider_handle::DividerDrag::claims` so a
// press in a divider's ±grab-zone never double-dispatches into the pane (or
// its webview) under the cursor.
pub(crate) mod divider_handle;
// NOTE: `modal` must be `pub(crate)`, not the repo's default private
// submodule, because `resolve_key_effects` (`src/input/keyboard/handler.rs`),
// `read_ime_events` (`src/input/ime.rs`), and `apply_type`
// (`src/input/shortcuts/apply.rs`) all call `modal::any_modal_open` to keep
// typing, shortcuts, paste, and IME commits out of the focused pane while a
// prompt owns the keyboard.
pub(crate) mod modal;
mod pane_focus;
// NOTE: `rename_prompt` must be `pub(crate)`, not the repo's default private
// submodule, because `apply_type` (`src/input/shortcuts/apply.rs`) gates on
// `rename_prompt::RenameState` to keep typed rename characters out of the PTY.
pub(crate) mod rename_prompt;
mod window_bar;

/// Root of the multiplexer UI subtree, mounted once under `UiRoot`.
#[derive(Component)]
struct MultiplexerUiRoot;

/// The always-visible window-bar row.
#[derive(Component)]
struct WindowBarContainer;

/// The area that hosts each window's pane containers (`flex_grow: 1`).
#[derive(Component)]
pub(crate) struct WorkspaceContainer;

/// Ensures the permanent multiplexer UI subtree exists under `UiRoot`.
pub(crate) struct MultiplexerUiPlugin;

impl Plugin for MultiplexerUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            ensure_ui_root.run_if(not(any_with_component::<MultiplexerUiRoot>)),
        )
        .add_plugins((
            confirm_prompt::ConfirmPromptPlugin,
            divider_handle::DividerHandlePlugin,
            pane_focus::PaneFocusPlugin,
            rename_prompt::RenamePromptPlugin,
            window_bar::WindowBarPlugin,
        ));
    }
}

fn ensure_ui_root(mut commands: Commands, ui_root: Query<Entity, With<UiRoot>>) {
    let Ok(ui_root) = ui_root.single() else {
        return;
    };
    let root = commands
        .spawn((
            Name::new("Multiplexer UI Root"),
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            MultiplexerUiRoot,
            ChildOf(ui_root),
        ))
        .id();
    commands.spawn((
        Name::new("Window Bar"),
        // NOTE: a fixed height keeps the always-visible bar from collapsing to
        // zero while it has no entries (PR-1); a later task fills it.
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(24.0),
            flex_shrink: 0.0,
            ..default()
        },
        WindowBarContainer,
        ChildOf(root),
    ));
    commands.spawn((
        Name::new("Workspace"),
        Node {
            flex_grow: 1.0,
            width: Val::Percent(100.0),
            ..default()
        },
        WorkspaceContainer,
        ChildOf(root),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins(MultiplexerUiPlugin);
        app
    }

    #[test]
    fn spawns_ui_root_once() {
        let mut app = build_app();
        app.update();
        let world = app.world_mut();
        let mut root = world.query_filtered::<(), With<MultiplexerUiRoot>>();
        assert_eq!(root.iter(world).count(), 1, "exactly one MultiplexerUiRoot");

        app.update();
        let world = app.world_mut();
        let mut root = world.query_filtered::<(), With<MultiplexerUiRoot>>();
        assert_eq!(
            root.iter(world).count(),
            1,
            "still exactly one MultiplexerUiRoot after second update"
        );
        let world = app.world_mut();
        let mut bar = world.query_filtered::<(), With<WindowBarContainer>>();
        assert_eq!(bar.iter(world).count(), 1, "exactly one WindowBarContainer");
        let world = app.world_mut();
        let mut workspace = world.query_filtered::<(), With<WorkspaceContainer>>();
        assert_eq!(
            workspace.iter(world).count(),
            1,
            "exactly one WorkspaceContainer"
        );
    }
}
