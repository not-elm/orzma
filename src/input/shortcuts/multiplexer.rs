//! The multiplexer shortcut applier: maps the 11 multiplexer `Shortcut` arms
//! (pane split/select/resize/zoom/kill; window new/kill/next/previous/select/
//! rename) to the centralized request events in
//! `crate::multiplexer::request`.

use crate::input::shortcuts::{ShortcutMessage, ShortcutSet};
use crate::multiplexer::layout::SplitAxis;
use crate::multiplexer::request::{
    NewWindowRequest, OpenKillPaneConfirm, OpenKillWindowConfirm, RenameWindowRequest,
    ResizePaneRequest, SelectPaneRequest, SelectWindowRequest, SplitPaneRequest, WindowSelect,
    ZoomPaneRequest,
};
use crate::multiplexer::window::ActiveMultiplexerWindow;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use orzma_configs::shortcuts::{Shortcut, SplitOrientation};

/// Registers `apply_multiplexer_shortcuts` and the multiplexer request
/// `Message` types it writes.
pub(super) struct MultiplexerShortcutPlugin;

impl Plugin for MultiplexerShortcutPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SelectPaneRequest>()
            .add_message::<ResizePaneRequest>()
            .add_message::<ZoomPaneRequest>()
            .add_message::<NewWindowRequest>()
            .add_message::<SelectWindowRequest>()
            .add_message::<RenameWindowRequest>()
            .add_message::<OpenKillPaneConfirm>()
            .add_message::<OpenKillWindowConfirm>()
            .add_systems(
                Update,
                apply_multiplexer_shortcuts
                    .in_set(ShortcutSet::Apply)
                    .run_if(on_message::<ShortcutMessage>),
            );
    }
}

/// The multiplexer request-event writers `apply_multiplexer_shortcuts` fans
/// out to, bundled to stay within Bevy's system-parameter limit.
#[derive(SystemParam)]
struct MultiplexerRequests<'w> {
    select_pane: MessageWriter<'w, SelectPaneRequest>,
    resize_pane: MessageWriter<'w, ResizePaneRequest>,
    zoom_pane: MessageWriter<'w, ZoomPaneRequest>,
    confirm_kill_pane: MessageWriter<'w, OpenKillPaneConfirm>,
    new_window: MessageWriter<'w, NewWindowRequest>,
    confirm_kill_window: MessageWriter<'w, OpenKillWindowConfirm>,
    select_window: MessageWriter<'w, SelectWindowRequest>,
    rename_window: MessageWriter<'w, RenameWindowRequest>,
}

/// Applies multiplexer-scoped keyboard shortcuts from `ShortcutMessage`:
/// `SplitPane` triggers `SplitPaneRequest` on the focused pane; `KillPane`
/// and `KillWindow` open their confirm prompts (`OpenKillPaneConfirm` /
/// `OpenKillWindowConfirm`, consumed by the Task 13 prompt) rather than
/// closing directly; the remaining pane/window arms write their `Message`
/// requests. The five non-multiplexer arms (`Paste`/`Copy`/`EnterViMode`/
/// `Quit`/`ReleaseWebviewFocus`) are handled by
/// `crate::input::shortcuts::apply`.
/// Registered in `ShortcutSet::Apply`, gated on `on_message::<ShortcutMessage>`.
fn apply_multiplexer_shortcuts(
    mut commands: Commands,
    mut shortcuts: MessageReader<ShortcutMessage>,
    mut requests: MultiplexerRequests,
    active_window: Query<Entity, With<ActiveMultiplexerWindow>>,
) {
    for msg in shortcuts.read() {
        match msg.action {
            Shortcut::SplitPane(orientation) => {
                if let Some(pane) = msg.focused {
                    commands.trigger(SplitPaneRequest {
                        pane,
                        axis: match orientation {
                            SplitOrientation::Vertical => SplitAxis::Vertical,
                            SplitOrientation::Horizontal => SplitAxis::Horizontal,
                        },
                    });
                }
            }
            Shortcut::SelectPane(dir) => {
                requests.select_pane.write(SelectPaneRequest { dir });
            }
            Shortcut::ResizePane(dir) => {
                requests.resize_pane.write(ResizePaneRequest { dir });
            }
            Shortcut::ZoomPane => {
                requests.zoom_pane.write(ZoomPaneRequest);
            }
            Shortcut::KillPane => {
                if let Some(pane) = msg.focused {
                    requests
                        .confirm_kill_pane
                        .write(OpenKillPaneConfirm { pane });
                }
            }
            Shortcut::NewWindow => {
                requests.new_window.write(NewWindowRequest);
            }
            Shortcut::KillWindow => {
                if let Ok(window) = active_window.single() {
                    requests
                        .confirm_kill_window
                        .write(OpenKillWindowConfirm { window });
                }
            }
            Shortcut::NextWindow => {
                requests
                    .select_window
                    .write(SelectWindowRequest(WindowSelect::Next));
            }
            Shortcut::PreviousWindow => {
                requests
                    .select_window
                    .write(SelectWindowRequest(WindowSelect::Previous));
            }
            Shortcut::SelectWindow(index) => {
                requests
                    .select_window
                    .write(SelectWindowRequest(WindowSelect::Index(index)));
            }
            Shortcut::RenameWindow => {
                requests.rename_window.write(RenameWindowRequest);
            }
            Shortcut::Paste
            | Shortcut::Copy
            | Shortcut::ReleaseWebviewFocus
            | Shortcut::Quit
            | Shortcut::EnterViMode => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_configs::shortcuts::PaneDirection;

    #[derive(Resource, Default)]
    struct Captured {
        split_targets: Vec<(Entity, SplitAxis)>,
        select_pane: Vec<PaneDirection>,
        resize_pane: Vec<PaneDirection>,
        zoom_pane: u32,
        confirm_kill_pane: Vec<Entity>,
        new_window: u32,
        confirm_kill_window: Vec<Entity>,
        select_window: Vec<WindowSelect>,
        rename_window: u32,
    }

    fn capture_select_pane(mut reader: MessageReader<SelectPaneRequest>, mut c: ResMut<Captured>) {
        for m in reader.read() {
            c.select_pane.push(m.dir);
        }
    }

    fn capture_resize_pane(mut reader: MessageReader<ResizePaneRequest>, mut c: ResMut<Captured>) {
        for m in reader.read() {
            c.resize_pane.push(m.dir);
        }
    }

    fn capture_zoom_pane(mut reader: MessageReader<ZoomPaneRequest>, mut c: ResMut<Captured>) {
        c.zoom_pane += reader.read().count() as u32;
    }

    fn capture_confirm_kill_pane(
        mut reader: MessageReader<OpenKillPaneConfirm>,
        mut c: ResMut<Captured>,
    ) {
        for m in reader.read() {
            c.confirm_kill_pane.push(m.pane);
        }
    }

    fn capture_new_window(mut reader: MessageReader<NewWindowRequest>, mut c: ResMut<Captured>) {
        c.new_window += reader.read().count() as u32;
    }

    fn capture_confirm_kill_window(
        mut reader: MessageReader<OpenKillWindowConfirm>,
        mut c: ResMut<Captured>,
    ) {
        for m in reader.read() {
            c.confirm_kill_window.push(m.window);
        }
    }

    fn capture_select_window(
        mut reader: MessageReader<SelectWindowRequest>,
        mut c: ResMut<Captured>,
    ) {
        for m in reader.read() {
            c.select_window.push(m.0);
        }
    }

    fn capture_rename_window(
        mut reader: MessageReader<RenameWindowRequest>,
        mut c: ResMut<Captured>,
    ) {
        c.rename_window += reader.read().count() as u32;
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<ShortcutMessage>()
            .add_message::<SelectPaneRequest>()
            .add_message::<ResizePaneRequest>()
            .add_message::<ZoomPaneRequest>()
            .add_message::<NewWindowRequest>()
            .add_message::<SelectWindowRequest>()
            .add_message::<RenameWindowRequest>()
            .add_message::<OpenKillPaneConfirm>()
            .add_message::<OpenKillWindowConfirm>()
            .init_resource::<Captured>()
            .add_systems(Update, apply_multiplexer_shortcuts)
            .add_systems(
                Update,
                (
                    capture_select_pane,
                    capture_resize_pane,
                    capture_zoom_pane,
                    capture_confirm_kill_pane,
                    capture_new_window,
                    capture_confirm_kill_window,
                    capture_select_window,
                    capture_rename_window,
                )
                    .after(apply_multiplexer_shortcuts),
            )
            .add_observer(|ev: On<SplitPaneRequest>, mut captured: ResMut<Captured>| {
                captured.split_targets.push((ev.pane, ev.axis));
            });
        app
    }

    fn write_shortcut(app: &mut App, action: Shortcut, focused: Option<Entity>) {
        app.world_mut().write_message(ShortcutMessage {
            action,
            via_leader: false,
            focused,
            in_vi_mode: false,
        });
        app.update();
    }

    #[test]
    fn split_pane_shortcut_triggers_split_pane_request_on_focused_pane() {
        let mut app = test_app();
        let pane = app.world_mut().spawn_empty().id();
        write_shortcut(
            &mut app,
            Shortcut::SplitPane(SplitOrientation::Vertical),
            Some(pane),
        );
        assert_eq!(
            app.world().resource::<Captured>().split_targets,
            vec![(pane, SplitAxis::Vertical)],
            "SplitPane(Vertical) must trigger SplitPaneRequest on the focused pane with SplitAxis::Vertical"
        );
    }

    #[test]
    fn split_pane_shortcut_maps_horizontal_orientation() {
        let mut app = test_app();
        let pane = app.world_mut().spawn_empty().id();
        write_shortcut(
            &mut app,
            Shortcut::SplitPane(SplitOrientation::Horizontal),
            Some(pane),
        );
        assert_eq!(
            app.world().resource::<Captured>().split_targets,
            vec![(pane, SplitAxis::Horizontal)],
            "SplitOrientation::Horizontal must map to SplitAxis::Horizontal"
        );
    }

    #[test]
    fn split_pane_shortcut_with_no_focus_is_noop() {
        let mut app = test_app();
        write_shortcut(
            &mut app,
            Shortcut::SplitPane(SplitOrientation::Vertical),
            None,
        );
        assert!(
            app.world().resource::<Captured>().split_targets.is_empty(),
            "no focused pane means no SplitPaneRequest"
        );
    }

    #[test]
    fn select_pane_shortcut_writes_select_pane_request() {
        let mut app = test_app();
        write_shortcut(&mut app, Shortcut::SelectPane(PaneDirection::Left), None);
        assert_eq!(
            app.world().resource::<Captured>().select_pane,
            vec![PaneDirection::Left]
        );
    }

    #[test]
    fn resize_pane_shortcut_writes_resize_pane_request() {
        let mut app = test_app();
        write_shortcut(&mut app, Shortcut::ResizePane(PaneDirection::Right), None);
        assert_eq!(
            app.world().resource::<Captured>().resize_pane,
            vec![PaneDirection::Right]
        );
    }

    #[test]
    fn zoom_pane_shortcut_writes_zoom_pane_request() {
        let mut app = test_app();
        write_shortcut(&mut app, Shortcut::ZoomPane, None);
        assert_eq!(app.world().resource::<Captured>().zoom_pane, 1);
    }

    #[test]
    fn kill_pane_shortcut_opens_confirm_for_focused_pane() {
        let mut app = test_app();
        let pane = app.world_mut().spawn_empty().id();
        write_shortcut(&mut app, Shortcut::KillPane, Some(pane));
        assert_eq!(
            app.world().resource::<Captured>().confirm_kill_pane,
            vec![pane],
            "KillPane must open the confirm prompt for the focused pane, not close it directly"
        );
    }

    #[test]
    fn kill_pane_shortcut_with_no_focus_is_noop() {
        let mut app = test_app();
        write_shortcut(&mut app, Shortcut::KillPane, None);
        assert!(
            app.world()
                .resource::<Captured>()
                .confirm_kill_pane
                .is_empty()
        );
    }

    #[test]
    fn new_window_shortcut_writes_new_window_request() {
        let mut app = test_app();
        write_shortcut(&mut app, Shortcut::NewWindow, None);
        assert_eq!(app.world().resource::<Captured>().new_window, 1);
    }

    #[test]
    fn kill_window_shortcut_opens_confirm_for_active_window() {
        let mut app = test_app();
        let window = app.world_mut().spawn(ActiveMultiplexerWindow).id();
        write_shortcut(&mut app, Shortcut::KillWindow, None);
        assert_eq!(
            app.world().resource::<Captured>().confirm_kill_window,
            vec![window],
            "KillWindow must open the confirm prompt for the active window"
        );
    }

    #[test]
    fn kill_window_shortcut_with_no_active_window_is_noop() {
        let mut app = test_app();
        write_shortcut(&mut app, Shortcut::KillWindow, None);
        assert!(
            app.world()
                .resource::<Captured>()
                .confirm_kill_window
                .is_empty()
        );
    }

    #[test]
    fn next_previous_and_select_window_map_to_select_window_request() {
        let mut app = test_app();
        write_shortcut(&mut app, Shortcut::NextWindow, None);
        write_shortcut(&mut app, Shortcut::PreviousWindow, None);
        write_shortcut(&mut app, Shortcut::SelectWindow(3), None);
        let selected = &app.world().resource::<Captured>().select_window;
        assert_eq!(selected.len(), 3);
        assert!(matches!(selected[0], WindowSelect::Next));
        assert!(matches!(selected[1], WindowSelect::Previous));
        assert!(matches!(selected[2], WindowSelect::Index(3)));
    }

    #[test]
    fn rename_window_shortcut_writes_rename_window_request() {
        let mut app = test_app();
        write_shortcut(&mut app, Shortcut::RenameWindow, None);
        assert_eq!(app.world().resource::<Captured>().rename_window, 1);
    }

    #[test]
    fn non_multiplexer_shortcut_arms_are_noop() {
        let mut app = test_app();
        let pane = app.world_mut().spawn_empty().id();
        for action in [
            Shortcut::Paste,
            Shortcut::Copy,
            Shortcut::ReleaseWebviewFocus,
            Shortcut::Quit,
            Shortcut::EnterViMode,
        ] {
            write_shortcut(&mut app, action, Some(pane));
        }
        let c = app.world().resource::<Captured>();
        assert!(c.split_targets.is_empty());
        assert!(c.select_pane.is_empty());
        assert!(c.resize_pane.is_empty());
        assert_eq!(c.zoom_pane, 0);
        assert!(c.confirm_kill_pane.is_empty());
        assert_eq!(c.new_window, 0);
        assert!(c.confirm_kill_window.is_empty());
        assert!(c.select_window.is_empty());
        assert_eq!(c.rename_window, 0);
    }
}
