//! In-copy-mode key handling for `AppMode::Default`: a pure key→intent
//! decider, a gather system that reads `KeyboardInput` for the focused
//! copy-mode terminal, and an observer that applies each intent through the
//! engine's vi/selection/scroll API. Entry (`Cmd+S`) lives in
//! `app_shortcut_handler` (`src/default_input.rs`); this module owns only the
//! keys handled WHILE copy mode is active.

use crate::app_mode::AppMode;
use crate::default_input::should_disable_input;
use crate::input::InputPhase;
use crate::input::current_modifiers;
use crate::input::ime::ImeState;
use crate::ui::copy_mode::{CopyModeState, ExitCopyMode};
use bevy::ecs::message::MessageReader;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_terminal::{Clipboard, KeyboardFocused, OzmaTerminal};
use ozma_tty_engine::{Coalescer, SelectionType, TerminalHandle, ViMotion};
use ozmux_configs::shortcuts::Modifiers;

/// Registers the in-copy-mode key gather system and apply observer.
pub(crate) struct CopyModeInputPlugin;

impl Plugin for CopyModeInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            copy_mode_keys
                .in_set(InputPhase::FocusedKey)
                .run_if(in_state(AppMode::Default))
                .run_if(on_message::<KeyboardInput>),
        )
        .add_observer(on_copy_mode_key_action);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollKind {
    PageUp,
    PageDown,
    HalfUp,
    HalfDown,
    LineUp,
    LineDown,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionOp {
    Start(SelectionType),
    Change(SelectionType),
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyModeIntent {
    Motion(ViMotion),
    Scroll(ScrollKind),
    SelectionToggle(SelectionType),
    Yank,
    Exit,
}

#[derive(EntityEvent, Debug)]
struct CopyModeKeyAction {
    entity: Entity,
    intent: CopyModeIntent,
}

/// Resolves a `v`/`V` keypress into a selection op given the current
/// selection: same kind clears, a different kind switches, none starts.
fn resolve_selection_toggle(
    current: Option<SelectionType>,
    requested: SelectionType,
) -> SelectionOp {
    match current {
        Some(c) if c == requested => SelectionOp::Clear,
        Some(_) => SelectionOp::Change(requested),
        None => SelectionOp::Start(requested),
    }
}

/// Maps one keypress to a copy-mode intent, or `None` when the key is unbound
/// in copy mode. Pure (no world access). `meta`/`alt` chords are app shortcuts
/// (or Option-as-Meta), never vi keys.
fn decide_copy_mode_key(
    logical_key: &Key,
    key_code: KeyCode,
    mods: Modifiers,
) -> Option<CopyModeIntent> {
    if mods.meta || mods.alt {
        return None;
    }
    if mods.ctrl {
        return match key_code {
            KeyCode::KeyF => Some(CopyModeIntent::Scroll(ScrollKind::PageDown)),
            KeyCode::KeyB => Some(CopyModeIntent::Scroll(ScrollKind::PageUp)),
            KeyCode::KeyD => Some(CopyModeIntent::Scroll(ScrollKind::HalfDown)),
            KeyCode::KeyU => Some(CopyModeIntent::Scroll(ScrollKind::HalfUp)),
            KeyCode::KeyE => Some(CopyModeIntent::Scroll(ScrollKind::LineDown)),
            KeyCode::KeyY => Some(CopyModeIntent::Scroll(ScrollKind::LineUp)),
            KeyCode::KeyC => Some(CopyModeIntent::Exit),
            _ => None,
        };
    }
    match logical_key {
        Key::Escape => return Some(CopyModeIntent::Exit),
        Key::Enter => return Some(CopyModeIntent::Yank),
        Key::ArrowLeft => return Some(CopyModeIntent::Motion(ViMotion::Left)),
        Key::ArrowDown => return Some(CopyModeIntent::Motion(ViMotion::Down)),
        Key::ArrowUp => return Some(CopyModeIntent::Motion(ViMotion::Up)),
        Key::ArrowRight => return Some(CopyModeIntent::Motion(ViMotion::Right)),
        _ => {}
    }
    let Key::Character(s) = logical_key else {
        return None;
    };
    Some(match s.as_str() {
        "h" => CopyModeIntent::Motion(ViMotion::Left),
        "j" => CopyModeIntent::Motion(ViMotion::Down),
        "k" => CopyModeIntent::Motion(ViMotion::Up),
        "l" => CopyModeIntent::Motion(ViMotion::Right),
        "0" => CopyModeIntent::Motion(ViMotion::First),
        "$" => CopyModeIntent::Motion(ViMotion::Last),
        "^" => CopyModeIntent::Motion(ViMotion::FirstOccupied),
        "w" => CopyModeIntent::Motion(ViMotion::SemanticRight),
        "b" => CopyModeIntent::Motion(ViMotion::SemanticLeft),
        "e" => CopyModeIntent::Motion(ViMotion::SemanticRightEnd),
        "W" => CopyModeIntent::Motion(ViMotion::WordRight),
        "B" => CopyModeIntent::Motion(ViMotion::WordLeft),
        "E" => CopyModeIntent::Motion(ViMotion::WordRightEnd),
        "H" => CopyModeIntent::Motion(ViMotion::High),
        "M" => CopyModeIntent::Motion(ViMotion::Middle),
        "L" => CopyModeIntent::Motion(ViMotion::Low),
        "{" => CopyModeIntent::Motion(ViMotion::ParagraphUp),
        "}" => CopyModeIntent::Motion(ViMotion::ParagraphDown),
        "%" => CopyModeIntent::Motion(ViMotion::Bracket),
        "g" => CopyModeIntent::Scroll(ScrollKind::Top),
        "G" => CopyModeIntent::Scroll(ScrollKind::Bottom),
        "v" => CopyModeIntent::SelectionToggle(SelectionType::Simple),
        "V" => CopyModeIntent::SelectionToggle(SelectionType::Lines),
        "y" => CopyModeIntent::Yank,
        "q" => CopyModeIntent::Exit,
        _ => return None,
    })
}

/// Gather system: reads `KeyboardInput` for the focused copy-mode terminal,
/// decides an intent per key, and triggers `CopyModeKeyAction`. Suspends
/// (and drains events) while input is disabled (IME composing, webview focused,
/// or window unfocused) — the same coarse guard `maintain_input_gates` uses
/// (`should_disable_input`).
fn copy_mode_keys(
    mut commands: Commands,
    mut events: MessageReader<KeyboardInput>,
    ime: Res<ImeState>,
    focused_webview: Res<FocusedWebview>,
    bevy_keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    terminal: Query<
        Entity,
        (
            With<OzmaTerminal>,
            With<KeyboardFocused>,
            With<CopyModeState>,
        ),
    >,
) {
    let Ok(entity) = terminal.single() else {
        events.clear();
        return;
    };
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    if should_disable_input(ime.is_composing(), focused, focused_webview.0.is_some()) {
        events.clear();
        return;
    }
    let mods = current_modifiers(&bevy_keys);
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if let Some(intent) = decide_copy_mode_key(&ev.logical_key, ev.key_code, mods) {
            commands.trigger(CopyModeKeyAction { entity, intent });
        }
    }
}

/// Observer: applies a `CopyModeKeyAction` to the target terminal's engine
/// handle. The `v`/`V` toggle resolves against the LIVE selection at apply
/// time (observers for queued triggers run sequentially, so two same-frame
/// toggles see each other's effect). `Yank`/`Exit` additionally trigger
/// `ExitCopyMode`.
fn on_copy_mode_key_action(
    ev: On<CopyModeKeyAction>,
    mut commands: Commands,
    mut clipboard: ResMut<Clipboard>,
    mut terminals: Query<(&mut TerminalHandle, &mut Coalescer)>,
) {
    let Ok((mut handle, mut coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    match ev.intent {
        CopyModeIntent::Motion(m) => handle.vi_motion(&mut coalescer, m),
        CopyModeIntent::Scroll(kind) => apply_scroll(&mut handle, &mut coalescer, kind),
        CopyModeIntent::SelectionToggle(kind) => {
            let op = resolve_selection_toggle(handle.selection_type(), kind);
            apply_selection(&mut handle, &mut coalescer, op);
        }
        CopyModeIntent::Yank => {
            if let Some(text) = handle.selection_to_string() {
                clipboard.write(text);
            }
            commands.trigger(ExitCopyMode { entity: ev.entity });
        }
        CopyModeIntent::Exit => {
            commands.trigger(ExitCopyMode { entity: ev.entity });
        }
    }
}

/// Applies a scroll intent. Relative scrolls (half/line/page) move the vi
/// cursor with the viewport; `Top`/`Bottom` snap it to the buffer extremes.
fn apply_scroll(handle: &mut TerminalHandle, coalescer: &mut Coalescer, kind: ScrollKind) {
    match kind {
        ScrollKind::PageUp => handle.scroll_page_up(coalescer),
        ScrollKind::PageDown => handle.scroll_page_down(coalescer),
        ScrollKind::HalfUp => {
            let half = half_page(handle);
            handle.scroll(coalescer, half);
        }
        ScrollKind::HalfDown => {
            let half = half_page(handle);
            handle.scroll(coalescer, -half);
        }
        ScrollKind::LineUp => handle.scroll(coalescer, 1),
        ScrollKind::LineDown => handle.scroll(coalescer, -1),
        ScrollKind::Top => handle.scroll_to_top(coalescer),
        ScrollKind::Bottom => handle.scroll_to_bottom(coalescer),
    }
}

/// Half the visible row count (at least 1), for half-page scrolling. Read
/// lazily so the non-half scroll keys do not pay for a geometry read.
fn half_page(handle: &TerminalHandle) -> i32 {
    (handle.read_geometry().1 as i32 / 2).max(1)
}

/// Applies a selection op. `Change` falls back to `Start` when no selection
/// anchor exists (per `TerminalHandle::selection_change_type`).
fn apply_selection(handle: &mut TerminalHandle, coalescer: &mut Coalescer, op: SelectionOp) {
    match op {
        SelectionOp::Start(ty) => handle.selection_start(coalescer, ty),
        SelectionOp::Change(ty) => {
            if !handle.selection_change_type(coalescer, ty) {
                handle.selection_start(coalescer, ty);
            }
        }
        SelectionOp::Clear => handle.selection_clear(coalescer),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn mods(ctrl: bool, shift: bool, alt: bool, meta: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            alt,
            meta,
        }
    }

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn plain_motions_map_to_vi_motions() {
        let n = mods(false, false, false, false);
        assert_eq!(
            decide_copy_mode_key(&ch("h"), KeyCode::KeyH, n),
            Some(CopyModeIntent::Motion(ViMotion::Left))
        );
        assert_eq!(
            decide_copy_mode_key(&ch("$"), KeyCode::Digit4, n),
            Some(CopyModeIntent::Motion(ViMotion::Last))
        );
        assert_eq!(
            decide_copy_mode_key(&ch("w"), KeyCode::KeyW, n),
            Some(CopyModeIntent::Motion(ViMotion::SemanticRight))
        );
        assert_eq!(
            decide_copy_mode_key(&ch("W"), KeyCode::KeyW, mods(false, true, false, false)),
            Some(CopyModeIntent::Motion(ViMotion::WordRight))
        );
    }

    #[test]
    fn ctrl_combos_map_to_scroll_and_exit() {
        let c = mods(true, false, false, false);
        assert_eq!(
            decide_copy_mode_key(&ch("f"), KeyCode::KeyF, c),
            Some(CopyModeIntent::Scroll(ScrollKind::PageDown))
        );
        assert_eq!(
            decide_copy_mode_key(&ch("u"), KeyCode::KeyU, c),
            Some(CopyModeIntent::Scroll(ScrollKind::HalfUp))
        );
        assert_eq!(
            decide_copy_mode_key(&ch("c"), KeyCode::KeyC, c),
            Some(CopyModeIntent::Exit)
        );
    }

    #[test]
    fn g_and_shift_g_map_to_top_and_bottom() {
        let n = mods(false, false, false, false);
        assert_eq!(
            decide_copy_mode_key(&ch("g"), KeyCode::KeyG, n),
            Some(CopyModeIntent::Scroll(ScrollKind::Top))
        );
        assert_eq!(
            decide_copy_mode_key(&ch("G"), KeyCode::KeyG, mods(false, true, false, false)),
            Some(CopyModeIntent::Scroll(ScrollKind::Bottom))
        );
    }

    #[test]
    fn selection_keys_map_to_toggle() {
        let n = mods(false, false, false, false);
        assert_eq!(
            decide_copy_mode_key(&ch("v"), KeyCode::KeyV, n),
            Some(CopyModeIntent::SelectionToggle(SelectionType::Simple))
        );
        assert_eq!(
            decide_copy_mode_key(&ch("V"), KeyCode::KeyV, mods(false, true, false, false)),
            Some(CopyModeIntent::SelectionToggle(SelectionType::Lines))
        );
    }

    #[test]
    fn named_keys_map() {
        let n = mods(false, false, false, false);
        assert_eq!(
            decide_copy_mode_key(&Key::Escape, KeyCode::Escape, n),
            Some(CopyModeIntent::Exit)
        );
        assert_eq!(
            decide_copy_mode_key(&Key::Enter, KeyCode::Enter, n),
            Some(CopyModeIntent::Yank)
        );
        assert_eq!(
            decide_copy_mode_key(&Key::ArrowDown, KeyCode::ArrowDown, n),
            Some(CopyModeIntent::Motion(ViMotion::Down))
        );
    }

    #[test]
    fn meta_and_alt_chords_return_none() {
        assert_eq!(
            decide_copy_mode_key(&ch("q"), KeyCode::KeyQ, mods(false, false, false, true)),
            None
        );
        assert_eq!(
            decide_copy_mode_key(&ch("j"), KeyCode::KeyJ, mods(false, false, true, false)),
            None
        );
    }

    #[test]
    fn selection_toggle_resolves_against_current() {
        assert_eq!(
            resolve_selection_toggle(None, SelectionType::Simple),
            SelectionOp::Start(SelectionType::Simple)
        );
        assert_eq!(
            resolve_selection_toggle(Some(SelectionType::Simple), SelectionType::Simple),
            SelectionOp::Clear
        );
        assert_eq!(
            resolve_selection_toggle(Some(SelectionType::Lines), SelectionType::Simple),
            SelectionOp::Change(SelectionType::Simple)
        );
    }

    #[test]
    fn yank_and_exit_keys() {
        let n = mods(false, false, false, false);
        assert_eq!(
            decide_copy_mode_key(&ch("y"), KeyCode::KeyY, n),
            Some(CopyModeIntent::Yank)
        );
        assert_eq!(
            decide_copy_mode_key(&ch("q"), KeyCode::KeyQ, n),
            Some(CopyModeIntent::Exit)
        );
    }

    #[test]
    fn unbound_keys_return_none() {
        let n = mods(false, false, false, false);
        assert_eq!(decide_copy_mode_key(&ch("z"), KeyCode::KeyZ, n), None);
    }

    #[test]
    fn yank_writes_selection_to_clipboard_and_exits() {
        use crate::ui::copy_mode::{CopyModePlugin, EnterCopyModeActionEvent};
        use ozma_tty_engine::{SpawnOptions, TerminalBundle};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(CopyModePlugin);
        app.add_observer(on_copy_mode_key_action);

        let opts = SpawnOptions {
            cols: 20,
            rows: 5,
            shell: "/bin/sh".into(),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = TerminalBundle::spawn(opts).expect("spawn /bin/sh");
        let entity = app.world_mut().spawn(bundle).id();

        app.world_mut().trigger(EnterCopyModeActionEvent { entity });
        app.update();
        app.world_mut()
            .run_system_once(move |mut q: Query<(&mut TerminalHandle, &mut Coalescer)>| {
                let (mut h, mut c) = q.get_mut(entity).unwrap();
                h.advance(b"hello world");
                h.selection_start(&mut c, SelectionType::Simple);
                h.vi_motion(&mut c, ViMotion::Last);
            })
            .unwrap();

        app.world_mut().trigger(CopyModeKeyAction {
            entity,
            intent: CopyModeIntent::Yank,
        });
        app.update();

        assert!(app.world().get::<CopyModeState>(entity).is_none());
        let text = app.world_mut().resource_mut::<Clipboard>().read();
        assert!(
            text.is_some_and(|t| !t.is_empty()),
            "yank must populate the clipboard"
        );
    }
}
