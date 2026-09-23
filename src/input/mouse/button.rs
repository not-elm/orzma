//! Mouse-button and pointer-motion dispatch for every `OrzmaTerminal`: the
//! host keeps hit-testing, click counting, hyperlink opens, and click-to-focus,
//! and hands every other button and motion event to the pane's backend.

use super::{
    CellContext, HeldSurfaces, MouseEffect, TerminalSurfaces, cell_context_for, cell_dims,
    hit_candidates, on_any_mouse_message, protocol_mods, trigger_mouse_effect,
};
use crate::input::bindings::OrzmaMouseConfig;
use crate::input::current_modifiers;
use crate::input::focus::PaneClicked;
use crate::input::hyperlink::link_modifier_held;
use crate::input::keyboard::current_terminal_modifiers;
use crate::input::mouse::MousePhase;
use crate::input::mouse::gesture::{HeldPointer, OrzmaMouseGesture};
use crate::input::mouse::separator::GrabbedSeparator;
use crate::surface::geometry::topmost_surface_at;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::prelude::*;
use bevy::time::{Real, Time};
use bevy::window::{CursorMoved, PrimaryWindow, WindowFocused};
use bevy_orzma_tty_renderer::TerminalCellMetricsResource;
use bevy_orzmux::prelude::CellSide;
use orzma_tty::prelude::{CellCoord, PointerButton, PointerInput, PointerKind, ProtocolModifiers};
use std::time::Duration;

/// Adds mouse-button dispatch and its gesture resource.
pub(super) struct MouseButtonInputPlugin;

impl Plugin for MouseButtonInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OrzmaMouseGesture>().add_systems(
            Update,
            dispatch_mouse_buttons
                .in_set(MousePhase::Dispatch)
                .run_if(on_any_mouse_message().or_else(on_message::<WindowFocused>)),
        );
    }
}

/// Per-frame constants computed once and threaded into the per-event
/// helpers.
struct FrameContext {
    cursor_phys: Vec2,
    scale: f32,
    cell_w: f32,
    cell_h: f32,
    mods: ProtocolModifiers,
    modifier_held: bool,
}

/// The shared mouse-button dispatcher. Locks the gesture to the topmost
/// terminal under the cursor on the first press and hands every press,
/// motion, and release to the locked terminal's backend until no button
/// is held; with nothing held, motion goes to the terminal under the
/// cursor. Skips any `OrzmaTerminal` carrying `TerminalMouseDisabled` or
/// `MouseClaimedByWebview`, except that a webview claim does not unlock a
/// held gesture. A lost window focus, or a locked terminal that became
/// mouse-disabled or vanished, cancels the held gesture; a frame with no
/// usable cursor, or with no mouse-enabled terminal while nothing is held,
/// resets it. While a separator grab holds the mouse, the frame's button
/// and cursor messages are drained unseen.
fn dispatch_mouse_buttons(
    mut commands: Commands,
    mut gesture: ResMut<OrzmaMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut cursor_moved: MessageReader<CursorMoved>,
    terminals: TerminalSurfaces,
    held_surfaces: HeldSurfaces,
    grabs: Query<(), With<GrabbedSeparator>>,
    cfg: Res<OrzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    // NOTE: a separator grab must drain this system's readers rather than
    // skip the system: a skipped reader keeps the grab's press and release
    // unread, and a later frame would replay them into a pane as a click.
    if !grabs.is_empty() {
        buttons.clear();
        cursor_moved.clear();
        return;
    }
    let Some(frame) = resolve_frame(
        &mut gesture,
        &mut cursor_moved,
        &terminals,
        &windows,
        &metrics,
        &keys,
    ) else {
        buttons.clear();
        cursor_moved.clear();
        let mods = protocol_mods(&current_terminal_modifiers(&keys));
        abandon_gesture(&mut commands, &mut gesture, mods);
        return;
    };
    if gesture
        .held
        .is_some_and(|held| !held_surfaces.contains(held.entity))
    {
        abandon_gesture(&mut commands, &mut gesture, frame.mods);
    }
    let now = time.elapsed();
    for ev in buttons.read() {
        let Some(button) = pointer_button(ev.button) else {
            continue;
        };
        match ev.state {
            ButtonState::Pressed => press(
                &mut commands,
                &mut gesture,
                &terminals,
                &held_surfaces,
                &frame,
                &cfg,
                button,
                now,
            ),
            ButtonState::Released => {
                release(&mut commands, &mut gesture, &held_surfaces, &frame, button)
            }
        }
    }
    send_motion(
        &mut commands,
        &mut gesture,
        &terminals,
        &held_surfaces,
        &frame,
    );
}

/// Resolves the window guard and the per-frame cursor/constants for one
/// run, or `None` when the frame should be skipped: the window is missing
/// or unfocused, no surface can take the mouse and nothing is held, or no
/// cursor position is usable. It reads `cursor_moved` only to refresh
/// `last_cursor_phys` and leaves the rest of the gesture untouched.
fn resolve_frame(
    gesture: &mut OrzmaMouseGesture,
    cursor_moved: &mut MessageReader<CursorMoved>,
    terminals: &TerminalSurfaces<'_, '_>,
    windows: &Query<&Window, With<PrimaryWindow>>,
    metrics: &TerminalCellMetricsResource,
    keys: &ButtonInput<KeyCode>,
) -> Option<FrameContext> {
    let window = match windows.single() {
        Ok(window) if window.focused && (!terminals.is_empty() || gesture.held.is_some()) => window,
        _ => {
            return None;
        }
    };
    let scale = window.scale_factor();
    let moved_phys = cursor_moved.read().last().map(|m| m.position * scale);
    let live = window.cursor_position().map(|c| c * scale);
    if let Some(latest) = live.or(moved_phys)
        && gesture.last_cursor_phys != Some(latest)
    {
        gesture.last_cursor_phys = Some(latest);
    }
    let cursor_phys =
        effective_drag_cursor(live, gesture.held.is_some(), gesture.last_cursor_phys)?;
    let (cell_w, cell_h) = cell_dims(metrics);
    Some(FrameContext {
        cursor_phys,
        scale,
        cell_w,
        cell_h,
        mods: protocol_mods(&current_terminal_modifiers(keys)),
        modifier_held: link_modifier_held(&current_modifiers(keys)),
    })
}

/// Handles one press. A Cmd/Ctrl-click on a hyperlink opens it and goes no
/// further. Any other press focuses its terminal and reaches that
/// terminal's backend: the first press locks the gesture to the topmost
/// terminal under the cursor, and a press while another button is held
/// goes to the locked terminal.
fn press(
    commands: &mut Commands,
    gesture: &mut OrzmaMouseGesture,
    terminals: &TerminalSurfaces<'_, '_>,
    held_surfaces: &HeldSurfaces<'_, '_>,
    frame: &FrameContext,
    cfg: &OrzmaMouseConfig,
    button: PointerButton,
    now: Duration,
) {
    let (target, ctx) = match gesture.held {
        Some(held) => (
            held.entity,
            CellContext::held(held_surfaces, held.entity, frame.cell_w, frame.cell_h),
        ),
        None => {
            let Some(target) = topmost_surface_at(frame.cursor_phys, hit_candidates(terminals))
            else {
                return;
            };
            (
                target,
                cell_context_for(terminals, target, frame.cell_w, frame.cell_h),
            )
        }
    };
    let Some(ctx) = ctx else {
        return;
    };
    let Some((cell, side)) = ctx.hit(frame.cursor_phys) else {
        return;
    };
    let click_count = gesture.click.register(
        now,
        frame.cursor_phys / frame.scale,
        (cfg.double_click_timeout, cfg.click_drift_px),
    );
    if let Some(uri) = link_press(button, frame.modifier_held, gesture.held.is_some(), || {
        ctx.link_at(cell)
    }) {
        trigger_mouse_effect(commands, target, MouseEffect::OpenUri(uri));
        return;
    }
    // NOTE: `PaneClicked` must trigger before the press: its observer sends
    // `SelectPane` on the same ordered command channel, so the backend
    // focuses the pane (and writes any focus report) before it routes the
    // press.
    commands.trigger(PaneClicked { entity: target });
    gesture
        .held
        .get_or_insert(HeldPointer::new(target))
        .press(button);
    gesture.last_target = Some((target, cell));
    trigger_mouse_effect(
        commands,
        target,
        MouseEffect::Pointer(PointerInput {
            kind: PointerKind::Press,
            button: Some(button),
            cell,
            side,
            click_count,
            mods: frame.mods,
        }),
    );
}

/// Handles one release: hands it to the locked terminal's backend and
/// unlocks the gesture once no button is held. A release whose press the
/// gesture never took, such as one that opened a hyperlink, is ignored.
fn release(
    commands: &mut Commands,
    gesture: &mut OrzmaMouseGesture,
    held_surfaces: &HeldSurfaces<'_, '_>,
    frame: &FrameContext,
    button: PointerButton,
) {
    let Some(mut held) = gesture.held.filter(|held| held.holds(button)) else {
        return;
    };
    // NOTE: a release whose cell cannot be resolved (a degenerate node) must
    // still reach the backend at the last cell sent; otherwise its button
    // stays latched there and later motion replays as a drag.
    let Some((cell, side)) =
        CellContext::held(held_surfaces, held.entity, frame.cell_w, frame.cell_h)
            .and_then(|ctx| ctx.hit(frame.cursor_phys))
            .or_else(|| gesture.last_target.map(|(_, cell)| (cell, CellSide::Left)))
    else {
        return;
    };
    held.release(button);
    gesture.held = (!held.is_empty()).then_some(held);
    if gesture.held.is_none() {
        gesture.last_cursor_phys = None;
    }
    gesture.last_target = Some((held.entity, cell));
    trigger_mouse_effect(
        commands,
        held.entity,
        MouseEffect::Pointer(PointerInput {
            kind: PointerKind::Release,
            button: Some(button),
            cell,
            side,
            click_count: 1,
            mods: frame.mods,
        }),
    );
}

/// Sends a motion event when the pointer's terminal or cell changed since
/// the last pointer event: to the locked terminal while a button is held,
/// its cell pinned to the terminal's edge off the node, and otherwise to
/// the topmost terminal under the cursor. With nothing held and no
/// terminal under the cursor it sends nothing and forgets the last target.
fn send_motion(
    commands: &mut Commands,
    gesture: &mut OrzmaMouseGesture,
    terminals: &TerminalSurfaces<'_, '_>,
    held_surfaces: &HeldSurfaces<'_, '_>,
    frame: &FrameContext,
) {
    let hit = match gesture.held {
        Some(held) => CellContext::held(held_surfaces, held.entity, frame.cell_w, frame.cell_h)
            .and_then(|ctx| ctx.hit(frame.cursor_phys))
            .map(|(cell, side)| (held.entity, cell, side)),
        None => {
            topmost_surface_at(frame.cursor_phys, hit_candidates(terminals)).and_then(|target| {
                let (cell, side) = cell_context_for(terminals, target, frame.cell_w, frame.cell_h)?
                    .hit(frame.cursor_phys)?;
                Some((target, cell, side))
            })
        }
    };
    let Some((target, cell, side)) = hit else {
        if gesture.held.is_none() {
            gesture.last_target = None;
        }
        return;
    };
    if gesture.last_target == Some((target, cell)) {
        return;
    }
    gesture.last_target = Some((target, cell));
    trigger_mouse_effect(
        commands,
        target,
        MouseEffect::Pointer(PointerInput {
            kind: PointerKind::Motion,
            button: None,
            cell,
            side,
            click_count: 1,
            mods: frame.mods,
        }),
    );
}

/// Hands the locked terminal a `Cancel`, so its backend releases every
/// button it forwarded, then resets the gesture. A locked terminal that no
/// longer exists gets nothing; with nothing held this only resets.
fn abandon_gesture(
    commands: &mut Commands,
    gesture: &mut OrzmaMouseGesture,
    mods: ProtocolModifiers,
) {
    if let Some(held) = gesture.held
        && commands.get_entity(held.entity).is_ok()
    {
        let cell = gesture
            .last_target
            .map_or(CellCoord { col: 1, row: 1 }, |(_, cell)| cell);
        trigger_mouse_effect(
            commands,
            held.entity,
            MouseEffect::Pointer(PointerInput {
                kind: PointerKind::Cancel,
                button: None,
                cell,
                side: CellSide::Left,
                click_count: 1,
                mods,
            }),
        );
    }
    gesture.reset();
}

/// The hyperlink a press opens instead of reaching its terminal: only a
/// left press with the link modifier held, while no other button is held,
/// on a cell `link` resolves to a URI.
fn link_press(
    button: PointerButton,
    modifier_held: bool,
    holding: bool,
    link: impl FnOnce() -> Option<String>,
) -> Option<String> {
    (button == PointerButton::Left && modifier_held && !holding)
        .then(link)
        .flatten()
}

/// The physical cursor position to drive the gesture with this frame.
///
/// `live` is `window.cursor_position()` (already `None` once the pointer
/// leaves the window, since Bevy bounds-masks off-window positions);
/// `active` is whether a button is held; `last` is the last observed
/// physical position. Returns the live position when present, the
/// last-known position while a button is held (so an off-window drag keeps
/// extending), or `None` when no button is held and the pointer is off the
/// window.
fn effective_drag_cursor(live: Option<Vec2>, active: bool, last: Option<Vec2>) -> Option<Vec2> {
    match (live, active) {
        (Some(c), _) => Some(c),
        (None, true) => last,
        (None, false) => None,
    }
}

fn pointer_button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Middle => Some(PointerButton::Middle),
        MouseButton::Right => Some(PointerButton::Right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::terminal::TerminalOpenUri;
    use crate::input::focus::{MouseClaimedByWebview, TerminalMouseDisabled};
    use crate::input::mouse::test_support::{set_phys_cursor, test_metrics};
    use crate::surface::OrzmaTerminal;
    use bevy::ecs::message::Messages;
    use bevy::input::mouse::MouseWheel;
    use bevy::ui::{ComputedNode, UiGlobalTransform};
    use bevy::window::WindowResolution;
    use bevy_orzma_tty_renderer::schema::TerminalView;
    use bevy_orzmux::prelude::RequestTtyPointer;

    /// What reached the world, in trigger order.
    #[derive(Debug, Clone, PartialEq)]
    enum Heard {
        Clicked(Entity),
        Pointer(Entity, PointerInput),
        Opened(String),
    }

    #[derive(Resource, Default)]
    struct Log(Vec<Heard>);

    impl Log {
        fn pointers(&self) -> Vec<(Entity, PointerInput)> {
            self.0
                .iter()
                .filter_map(|heard| match heard {
                    Heard::Pointer(entity, input) => Some((*entity, *input)),
                    Heard::Clicked(_) | Heard::Opened(_) => None,
                })
                .collect()
        }
    }

    /// A focused 800x600 window with the button plugin under its real run
    /// conditions, logging pane clicks, pointer events, and link opens.
    fn pointer_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>()
            .add_message::<CursorMoved>()
            .add_message::<MouseWheel>()
            .add_message::<WindowFocused>()
            .init_resource::<OrzmaMouseConfig>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Log>()
            .insert_resource(test_metrics())
            .add_plugins(MouseButtonInputPlugin)
            .add_observer(|ev: On<PaneClicked>, mut log: ResMut<Log>| {
                log.0.push(Heard::Clicked(ev.entity));
            })
            .add_observer(|ev: On<RequestTtyPointer>, mut log: ResMut<Log>| {
                log.0.push(Heard::Pointer(ev.terminal, ev.input));
            })
            .add_observer(|ev: On<TerminalOpenUri>, mut log: ResMut<Log>| {
                log.0.push(Heard::Opened(ev.uri.clone()));
            });
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        app
    }

    /// Spawns a terminal pane `width` physical px wide and 600 tall, with
    /// its left edge at `left`, at 8x16 px cells.
    fn spawn_pane(app: &mut App, left: f32, width: f32) -> Entity {
        app.world_mut()
            .spawn((
                OrzmaTerminal,
                ComputedNode {
                    size: Vec2::new(width, 600.0),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(left + width / 2.0, 300.0),
                TerminalView {
                    cols: (width / 8.0) as u16,
                    rows: 37,
                    ..default()
                },
            ))
            .id()
    }

    fn write_button(app: &mut App, button: MouseButton, state: ButtonState) {
        app.world_mut()
            .resource_mut::<Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button,
                state,
                window: Entity::PLACEHOLDER,
            });
    }

    /// Puts the cursor at `phys` and writes the `CursorMoved` a real move
    /// produces; a position off the window leaves only the message.
    fn move_to(app: &mut App, phys: Vec2) {
        set_phys_cursor(app, phys);
        app.world_mut()
            .resource_mut::<Messages<CursorMoved>>()
            .write(CursorMoved {
                window: Entity::PLACEHOLDER,
                position: phys,
                delta: None,
            });
    }

    /// Unfocuses the primary window and writes the `WindowFocused` a real
    /// focus change produces, with no mouse message alongside it.
    fn lose_focus(app: &mut App) {
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .expect("pointer_app spawns one primary window");
        app.world_mut()
            .get_mut::<Window>(window)
            .expect("the primary window")
            .focused = false;
        app.world_mut()
            .resource_mut::<Messages<WindowFocused>>()
            .write(WindowFocused {
                window,
                focused: false,
            });
    }

    fn cell(col: u32, row: u32) -> CellCoord {
        CellCoord { col, row }
    }

    fn last_pointer(app: &App) -> (Entity, PointerInput) {
        *app.world()
            .resource::<Log>()
            .pointers()
            .last()
            .expect("at least one pointer event")
    }

    /// Asserts that a press triggers `PaneClicked` on its pane before it
    /// hands the pane a `Press` carrying the cell under the cursor and a
    /// click count of one.
    ///
    /// Case: the user clicks once inside nvim's window in an inactive
    /// pane.
    #[test]
    fn a_press_focuses_its_pane_then_hands_it_the_press() {
        let mut app = pointer_app();
        let pane = spawn_pane(&mut app, 0.0, 800.0);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        let log = &app.world().resource::<Log>().0;
        assert_eq!(log.len(), 2, "{log:?}");
        assert_eq!(log[0], Heard::Clicked(pane));
        let Heard::Pointer(entity, input) = &log[1] else {
            panic!("expected a pointer event, got {log:?}");
        };
        assert_eq!(*entity, pane);
        assert_eq!(input.kind, PointerKind::Press);
        assert_eq!(input.button, Some(PointerButton::Left));
        assert_eq!(input.cell, cell(6, 4));
        assert_eq!(input.click_count, 1);
    }

    /// Asserts that a second press within the double-click window carries
    /// a click count of two.
    ///
    /// Case: the user double-clicks a word at a shell prompt.
    #[test]
    fn a_quick_second_press_carries_a_click_count_of_two() {
        let mut app = pointer_app();
        spawn_pane(&mut app, 0.0, 800.0);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        write_button(&mut app, MouseButton::Left, ButtonState::Released);
        app.update();
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        let pointers = app.world().resource::<Log>().pointers();
        assert_eq!(pointers.len(), 3, "{pointers:?}");
        assert_eq!(pointers[2].1.kind, PointerKind::Press);
        assert_eq!(pointers[2].1.click_count, 2);
    }

    /// Asserts that motion reaches the pane only when the cursor enters
    /// another cell.
    ///
    /// Case: the user drags across nvim's buffer, first jittering inside
    /// one cell.
    #[test]
    fn motion_is_sent_only_on_a_cell_change() {
        let mut app = pointer_app();
        let pane = spawn_pane(&mut app, 0.0, 800.0);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        move_to(&mut app, Vec2::new(43.0, 50.0));
        app.update();
        assert_eq!(app.world().resource::<Log>().pointers().len(), 1);
        move_to(&mut app, Vec2::new(48.0, 50.0));
        app.update();
        let (entity, input) = last_pointer(&app);
        assert_eq!(entity, pane);
        assert_eq!(input.kind, PointerKind::Motion);
        assert_eq!(input.button, None);
        assert_eq!(input.cell, cell(7, 4));
    }

    /// Asserts that a drag and a release that end over another pane still
    /// go to the pressed pane, with the cell pinned to its edge, and that
    /// the release unlocks the gesture so only hover motion reaches the
    /// other pane.
    ///
    /// Case: the user drags a visual selection in nvim's left pane past the
    /// divider into the right pane and lets go there.
    #[test]
    fn a_drag_and_release_stay_with_the_pressed_pane() {
        let mut app = pointer_app();
        let left = spawn_pane(&mut app, 0.0, 400.0);
        spawn_pane(&mut app, 400.0, 400.0);
        set_phys_cursor(&mut app, Vec2::new(44.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        move_to(&mut app, Vec2::new(440.0, 48.0));
        app.update();
        write_button(&mut app, MouseButton::Left, ButtonState::Released);
        app.update();
        let pointers = app.world().resource::<Log>().pointers();
        let to_left: Vec<(PointerKind, CellCoord)> = pointers
            .iter()
            .filter(|(entity, _)| *entity == left)
            .map(|(_, input)| (input.kind, input.cell))
            .collect();
        assert_eq!(
            to_left,
            vec![
                (PointerKind::Press, cell(6, 4)),
                (PointerKind::Motion, cell(50, 4)),
                (PointerKind::Release, cell(50, 4)),
            ]
        );
        assert!(
            pointers
                .iter()
                .all(|(entity, input)| *entity == left || input.kind == PointerKind::Motion),
            "{pointers:?}"
        );
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_none());
    }

    /// Asserts that with no button held, motion goes to the pane under the
    /// cursor as a buttonless event, only on a cell change, and nothing is
    /// sent while the cursor is over no pane.
    ///
    /// Case: the user moves the pointer over nvim with `mousemoveevent`
    /// set, out over empty window space, and back.
    #[test]
    fn hover_motion_goes_to_the_pane_under_the_cursor() {
        let mut app = pointer_app();
        let pane = spawn_pane(&mut app, 0.0, 400.0);
        move_to(&mut app, Vec2::new(44.0, 48.0));
        app.update();
        move_to(&mut app, Vec2::new(45.0, 48.0));
        app.update();
        move_to(&mut app, Vec2::new(600.0, 48.0));
        app.update();
        move_to(&mut app, Vec2::new(44.0, 48.0));
        app.update();
        let pointers = app.world().resource::<Log>().pointers();
        assert_eq!(pointers.len(), 2, "{pointers:?}");
        for (entity, input) in pointers {
            assert_eq!(entity, pane);
            assert_eq!(input.kind, PointerKind::Motion);
            assert_eq!(input.button, None);
            assert_eq!(input.cell, cell(6, 4));
        }
    }

    /// Asserts that a press while another button is held goes to the pane
    /// the first press locked, that the lock ends only once every button
    /// is up, and that the other pane gets nothing but hover motion.
    ///
    /// Case: the user holds the left button in nvim's pane, drifts over the
    /// next pane, and clicks the right button.
    #[test]
    fn a_chorded_press_goes_to_the_locked_pane() {
        let mut app = pointer_app();
        let left = spawn_pane(&mut app, 0.0, 400.0);
        spawn_pane(&mut app, 400.0, 400.0);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        move_to(&mut app, Vec2::new(440.0, 48.0));
        app.update();
        write_button(&mut app, MouseButton::Right, ButtonState::Pressed);
        app.update();
        write_button(&mut app, MouseButton::Left, ButtonState::Released);
        app.update();
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_some());
        write_button(&mut app, MouseButton::Right, ButtonState::Released);
        app.update();
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_none());
        let log = &app.world().resource::<Log>().0;
        assert!(
            log.iter().all(|heard| match heard {
                Heard::Clicked(entity) => *entity == left,
                Heard::Pointer(entity, input) => {
                    *entity == left || input.kind == PointerKind::Motion
                }
                Heard::Opened(_) => false,
            }),
            "{log:?}"
        );
        let presses_to_left = log
            .iter()
            .filter(|heard| {
                matches!(heard, Heard::Pointer(entity, PointerInput { kind: PointerKind::Press, .. }) if *entity == left)
            })
            .count();
        assert_eq!(presses_to_left, 2, "{log:?}");
    }

    /// Asserts that only a left press with the link modifier held and no
    /// other button held opens a hyperlink, and only on a linked cell.
    ///
    /// Case: the user Ctrl-clicks an OSC 8 link in `ls --hyperlink` output,
    /// then tries the same with the right button and in the middle of a
    /// drag.
    #[test]
    fn only_a_modified_left_press_opens_a_link() {
        let uri = || Some("https://example.com".to_string());
        assert_eq!(
            link_press(PointerButton::Left, true, false, uri),
            Some("https://example.com".to_string())
        );
        assert_eq!(link_press(PointerButton::Left, false, false, uri), None);
        assert_eq!(link_press(PointerButton::Right, true, false, uri), None);
        assert_eq!(link_press(PointerButton::Left, true, true, uri), None);
        assert_eq!(link_press(PointerButton::Left, true, false, || None), None);
    }

    /// Asserts that a live cursor always wins, an off-window cursor falls
    /// back to the last known position only while a button is held, and an
    /// idle off-window cursor yields nothing.
    ///
    /// Case: the user drags out of the window and back, or simply parks
    /// the pointer outside the window with no button held.
    #[test]
    fn effective_drag_cursor_truth_table() {
        let live = Vec2::new(10.0, 10.0);
        let last = Vec2::new(99.0, 88.0);
        assert_eq!(
            effective_drag_cursor(Some(live), false, Some(last)),
            Some(live)
        );
        assert_eq!(
            effective_drag_cursor(Some(live), true, Some(last)),
            Some(live)
        );
        assert_eq!(effective_drag_cursor(None, true, Some(last)), Some(last));
        assert_eq!(effective_drag_cursor(None, true, None), None);
        assert_eq!(effective_drag_cursor(None, false, Some(last)), None);
    }

    /// Asserts that a held drag that leaves the window keeps its lock and
    /// reaches the pane at the last column and row, and that the release
    /// there still reaches the pane.
    ///
    /// Case: the user drags a selection in nvim past the window's
    /// bottom-right corner and lets go outside the window.
    #[test]
    fn a_drag_past_the_window_edge_pins_to_the_last_cell() {
        let mut app = pointer_app();
        let pane = spawn_pane(&mut app, 0.0, 800.0);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        move_to(&mut app, Vec2::new(900.0, 700.0));
        app.update();
        let (entity, input) = last_pointer(&app);
        assert_eq!(entity, pane);
        assert_eq!(
            (input.kind, input.cell),
            (PointerKind::Motion, cell(100, 37))
        );
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_some());
        write_button(&mut app, MouseButton::Left, ButtonState::Released);
        app.update();
        let (_, input) = last_pointer(&app);
        assert_eq!(
            (input.kind, input.cell),
            (PointerKind::Release, cell(100, 37))
        );
    }

    /// Asserts that cursor motion outside the window with no button held
    /// sends nothing and leaves the gesture reset.
    ///
    /// Case: the user moves the pointer across the desktop past the
    /// terminal window without pressing anything.
    #[test]
    fn idle_cursor_outside_window_sends_nothing() {
        let mut app = pointer_app();
        spawn_pane(&mut app, 0.0, 800.0);
        move_to(&mut app, Vec2::new(900.0, 700.0));
        app.update();
        assert!(app.world().resource::<Log>().0.is_empty());
        let gesture = app.world().resource::<OrzmaMouseGesture>();
        assert!(gesture.held.is_none() && gesture.last_cursor_phys.is_none());
    }

    /// Asserts that a pane carrying `TerminalMouseDisabled` gets neither a
    /// focus click nor a pointer event from a press.
    ///
    /// Case: the user clicks a pane whose mouse input is disabled because it
    /// is in vi mode.
    #[test]
    fn a_mouse_disabled_pane_gets_no_pointer_events() {
        let mut app = pointer_app();
        let pane = spawn_pane(&mut app, 0.0, 800.0);
        app.world_mut()
            .entity_mut(pane)
            .insert(TerminalMouseDisabled);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        assert!(app.world().resource::<Log>().0.is_empty());
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_none());
    }

    /// Asserts that a pane a webview claims gets no pointer event from a
    /// press that starts over the claim.
    ///
    /// Case: the user clicks a link inside a page mounted in the pane.
    #[test]
    fn a_webview_claimed_pane_gets_no_pointer_events() {
        let mut app = pointer_app();
        let pane = spawn_pane(&mut app, 0.0, 800.0);
        app.world_mut()
            .entity_mut(pane)
            .insert(MouseClaimedByWebview);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        assert!(app.world().resource::<Log>().0.is_empty());
    }

    /// Asserts that losing window focus with a button held cancels the
    /// gesture on its pane even in a frame with no mouse message.
    ///
    /// Case: the user is dragging in nvim and switches windows with Alt+Tab
    /// without moving the mouse.
    #[test]
    fn losing_window_focus_cancels_the_held_gesture() {
        let mut app = pointer_app();
        let pane = spawn_pane(&mut app, 0.0, 800.0);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        app.update();
        lose_focus(&mut app);
        app.update();
        let (entity, input) = last_pointer(&app);
        assert_eq!(entity, pane);
        assert_eq!(input.kind, PointerKind::Cancel);
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_none());
    }

    /// Asserts that the held pane becoming mouse-disabled cancels the
    /// gesture and sends no further motion to it.
    ///
    /// Case: the user is dragging in a pane when an IME composition starts
    /// in it and disables its mouse input.
    #[test]
    fn disabling_the_held_pane_cancels_the_gesture() {
        let mut app = pointer_app();
        let pane = spawn_pane(&mut app, 0.0, 800.0);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        app.world_mut()
            .entity_mut(pane)
            .insert(TerminalMouseDisabled);
        move_to(&mut app, Vec2::new(56.0, 48.0));
        app.update();
        let (entity, input) = last_pointer(&app);
        assert_eq!(entity, pane);
        assert_eq!(input.kind, PointerKind::Cancel);
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_none());
    }

    /// Asserts that a webview claim on the held pane does not cancel the
    /// drag, which keeps reaching the pane.
    ///
    /// Case: the user drags a selection in nvim across an inline web page
    /// mounted in the same pane.
    #[test]
    fn a_webview_claim_does_not_break_a_held_drag() {
        let mut app = pointer_app();
        let pane = spawn_pane(&mut app, 0.0, 800.0);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        app.world_mut()
            .entity_mut(pane)
            .insert(MouseClaimedByWebview);
        move_to(&mut app, Vec2::new(60.0, 48.0));
        app.update();
        let (entity, input) = last_pointer(&app);
        assert_eq!(entity, pane);
        assert_eq!((input.kind, input.cell), (PointerKind::Motion, cell(8, 4)));
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_some());
    }

    /// Asserts that a press and its release arriving in one frame reach
    /// the pane as a press and then a release, and nothing else.
    ///
    /// Case: the user clicks so quickly that both button messages land in
    /// the same frame.
    #[test]
    fn a_same_frame_click_hands_the_press_then_the_release() {
        let mut app = pointer_app();
        spawn_pane(&mut app, 0.0, 800.0);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        write_button(&mut app, MouseButton::Left, ButtonState::Released);
        app.update();
        let kinds: Vec<PointerKind> = app
            .world()
            .resource::<Log>()
            .pointers()
            .iter()
            .map(|(_, input)| input.kind)
            .collect();
        assert_eq!(kinds, vec![PointerKind::Press, PointerKind::Release]);
        assert!(app.world().resource::<OrzmaMouseGesture>().held.is_none());
    }

    /// Asserts that the held pane vanishing resets the gesture without an
    /// event on the dead entity, and that the next press reaches the pane
    /// under the cursor.
    ///
    /// Case: the shell in the pane the user is dragging in exits, and the
    /// user then clicks the remaining pane.
    #[test]
    fn closing_the_held_pane_resets_and_frees_the_next_press() {
        let mut app = pointer_app();
        let left = spawn_pane(&mut app, 0.0, 400.0);
        let right = spawn_pane(&mut app, 400.0, 400.0);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(&mut app, MouseButton::Left, ButtonState::Pressed);
        app.update();
        app.world_mut().despawn(left);
        set_phys_cursor(&mut app, Vec2::new(440.0, 48.0));
        write_button(&mut app, MouseButton::Right, ButtonState::Pressed);
        app.update();
        let log = &app.world().resource::<Log>().0;
        assert_eq!(
            log[log.len() - 2..]
                .iter()
                .map(|heard| match heard {
                    Heard::Clicked(entity) | Heard::Pointer(entity, _) => *entity,
                    Heard::Opened(_) => Entity::PLACEHOLDER,
                })
                .collect::<Vec<_>>(),
            vec![right, right],
            "{log:?}"
        );
        assert!(
            !log.iter().any(|heard| matches!(
                heard,
                Heard::Pointer(entity, PointerInput { kind: PointerKind::Cancel, .. }) if *entity == left
            )),
            "{log:?}"
        );
    }
}
