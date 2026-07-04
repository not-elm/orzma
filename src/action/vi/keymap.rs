//! Startup-resolved `[copy-mode]` key table: normalizes each configured key
//! into a lookup form (`Plain` logical char / `Named` / `Ctrl` physical key)
//! and maps a matched action to the shared VI events. Used by both modes'
//! copy-mode key gathers.

use crate::action::vi::{
    ViExitRequest, ViMotionRequest, ViScrollRequest, ViSelectionToggleRequest, ViYankRequest,
};
use crate::configs::OzmuxConfigsResource;
use bevy::input::keyboard::{Key, KeyCode};
use bevy::prelude::*;
use ozma_tty_engine::{SelectionType, ViMotion};
use ozmux_configs::copy_mode::{
    CopyModeAction, CopyModeBaseKey, CopyModeKey, CopyModeNamedKey, CopyMotion, CopyPromptDir,
    CopySelection,
};
use ozmux_configs::shortcuts::Modifiers;
use ozmux_tmux::PromptKind;
use std::collections::HashMap;

/// Registers the `Startup` resolution of the `[copy-mode]` table.
pub(super) struct CopyModeKeymapPlugin;

impl Plugin for CopyModeKeymapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ResolvedCopyModeKeys>()
            .add_systems(Startup, build_copy_mode_keys);
    }
}

/// The `[copy-mode]` table resolved to a lookup map. Built once at startup.
#[derive(Resource, Default, Debug)]
pub(crate) struct ResolvedCopyModeKeys(HashMap<ResolvedKey, CopyModeAction>);

impl ResolvedCopyModeKeys {
    /// Returns the action bound to one keypress, or `None` (unbound —
    /// swallowed by copy mode). `Cmd`/`Alt` chords are never copy-mode keys.
    pub(crate) fn resolve(
        &self,
        logical_key: &Key,
        key_code: KeyCode,
        mods: Modifiers,
    ) -> Option<CopyModeAction> {
        if mods.meta || mods.alt {
            return None;
        }
        if mods.ctrl {
            // NOTE: the Ctrl table keys on the physical KeyCode, which cannot
            // carry Shift — without this guard Ctrl+Shift+F would silently
            // alias to the Ctrl+F binding (e.g. the stock detach chord
            // Ctrl+Shift+D would scroll half a page).
            if mods.shift {
                return None;
            }
            return self.0.get(&ResolvedKey::Ctrl(key_code)).copied();
        }
        let key = match logical_key {
            Key::Character(s) => ResolvedKey::Plain(s.to_string()),
            Key::Escape => ResolvedKey::Named(CopyModeNamedKey::Escape),
            Key::Enter => ResolvedKey::Named(CopyModeNamedKey::Enter),
            Key::Space => ResolvedKey::Named(CopyModeNamedKey::Space),
            Key::Tab => ResolvedKey::Named(CopyModeNamedKey::Tab),
            Key::Backspace => ResolvedKey::Named(CopyModeNamedKey::Backspace),
            Key::ArrowUp => ResolvedKey::Named(CopyModeNamedKey::ArrowUp),
            Key::ArrowDown => ResolvedKey::Named(CopyModeNamedKey::ArrowDown),
            Key::ArrowLeft => ResolvedKey::Named(CopyModeNamedKey::ArrowLeft),
            Key::ArrowRight => ResolvedKey::Named(CopyModeNamedKey::ArrowRight),
            _ => return None,
        };
        self.0.get(&key).copied()
    }
}

/// Fires the VI event for a matched action on `entity`, converting the
/// config-crate vocabulary to engine types. Shared by both modes' gathers.
pub(crate) fn trigger_copy_mode_action(
    commands: &mut Commands,
    entity: Entity,
    action: CopyModeAction,
) {
    match action {
        CopyModeAction::Motion(m) => commands.trigger(ViMotionRequest {
            entity,
            motion: vi_motion(m),
        }),
        CopyModeAction::Scroll(s) => commands.trigger(ViScrollRequest { entity, kind: s }),
        CopyModeAction::Selection(s) => commands.trigger(ViSelectionToggleRequest {
            entity,
            ty: selection_type(s),
        }),
        CopyModeAction::Yank => commands.trigger(ViYankRequest { entity }),
        CopyModeAction::Exit => commands.trigger(ViExitRequest { entity }),
        // TODO: wire to the local search applier once it exists (v1 defers
        // local copy-mode search); until then these actions are swallowed.
        CopyModeAction::Prompt(_) => {}
        CopyModeAction::SearchStep(_) => {}
    }
}

/// A configured key normalized for runtime lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ResolvedKey {
    /// Exact logical character (case-sensitive).
    Plain(String),
    /// A named logical key.
    Named(CopyModeNamedKey),
    /// `Ctrl+` entry, matched on the physical key.
    Ctrl(KeyCode),
}

/// `Startup` system: resolves `[copy-mode]` into the lookup map. Config
/// validation already rejected duplicates, so plain inserts are safe.
fn build_copy_mode_keys(
    mut resolved: ResMut<ResolvedCopyModeKeys>,
    configs: Res<OzmuxConfigsResource>,
) {
    let mut map = HashMap::new();
    for (label, keys, action) in configs.copy_mode.bindings_iter() {
        for key in keys {
            match normalize(key) {
                Some(rk) => {
                    map.insert(rk, action);
                }
                None => tracing::warn!(
                    label,
                    key = %key,
                    "copy-mode key has no physical mapping; ignoring binding"
                ),
            }
        }
    }
    resolved.0 = map;
}

/// Normalizes a config key to its lookup form. `None` only for `Ctrl+` chars
/// outside the a-z/0-9 physical map (config validation already restricts to
/// ASCII alphanumerics, so this is defensive).
fn normalize(key: &CopyModeKey) -> Option<ResolvedKey> {
    match (&key.key, key.ctrl) {
        (CopyModeBaseKey::Char(c), false) => Some(ResolvedKey::Plain(c.clone())),
        (CopyModeBaseKey::Named(n), false) => Some(ResolvedKey::Named(*n)),
        (CopyModeBaseKey::Char(c), true) => {
            ctrl_char_keycode(c.chars().next()?).map(ResolvedKey::Ctrl)
        }
        (CopyModeBaseKey::Named(n), true) => Some(ResolvedKey::Ctrl(named_keycode(*n))),
    }
}

fn named_keycode(named: CopyModeNamedKey) -> KeyCode {
    match named {
        CopyModeNamedKey::Escape => KeyCode::Escape,
        CopyModeNamedKey::Enter => KeyCode::Enter,
        CopyModeNamedKey::Space => KeyCode::Space,
        CopyModeNamedKey::Tab => KeyCode::Tab,
        CopyModeNamedKey::Backspace => KeyCode::Backspace,
        CopyModeNamedKey::ArrowUp => KeyCode::ArrowUp,
        CopyModeNamedKey::ArrowDown => KeyCode::ArrowDown,
        CopyModeNamedKey::ArrowLeft => KeyCode::ArrowLeft,
        CopyModeNamedKey::ArrowRight => KeyCode::ArrowRight,
    }
}

fn ctrl_char_keycode(c: char) -> Option<KeyCode> {
    Some(match c {
        'a' => KeyCode::KeyA,
        'b' => KeyCode::KeyB,
        'c' => KeyCode::KeyC,
        'd' => KeyCode::KeyD,
        'e' => KeyCode::KeyE,
        'f' => KeyCode::KeyF,
        'g' => KeyCode::KeyG,
        'h' => KeyCode::KeyH,
        'i' => KeyCode::KeyI,
        'j' => KeyCode::KeyJ,
        'k' => KeyCode::KeyK,
        'l' => KeyCode::KeyL,
        'm' => KeyCode::KeyM,
        'n' => KeyCode::KeyN,
        'o' => KeyCode::KeyO,
        'p' => KeyCode::KeyP,
        'q' => KeyCode::KeyQ,
        'r' => KeyCode::KeyR,
        's' => KeyCode::KeyS,
        't' => KeyCode::KeyT,
        'u' => KeyCode::KeyU,
        'v' => KeyCode::KeyV,
        'w' => KeyCode::KeyW,
        'x' => KeyCode::KeyX,
        'y' => KeyCode::KeyY,
        'z' => KeyCode::KeyZ,
        '0' => KeyCode::Digit0,
        '1' => KeyCode::Digit1,
        '2' => KeyCode::Digit2,
        '3' => KeyCode::Digit3,
        '4' => KeyCode::Digit4,
        '5' => KeyCode::Digit5,
        '6' => KeyCode::Digit6,
        '7' => KeyCode::Digit7,
        '8' => KeyCode::Digit8,
        '9' => KeyCode::Digit9,
        _ => return None,
    })
}

fn vi_motion(motion: CopyMotion) -> ViMotion {
    match motion {
        CopyMotion::Left => ViMotion::Left,
        CopyMotion::Down => ViMotion::Down,
        CopyMotion::Up => ViMotion::Up,
        CopyMotion::Right => ViMotion::Right,
        CopyMotion::LineStart => ViMotion::First,
        CopyMotion::LineEnd => ViMotion::Last,
        CopyMotion::LineFirstChar => ViMotion::FirstOccupied,
        CopyMotion::NextWord => ViMotion::SemanticRight,
        CopyMotion::PreviousWord => ViMotion::SemanticLeft,
        CopyMotion::NextWordEnd => ViMotion::SemanticRightEnd,
        CopyMotion::NextSpace => ViMotion::WordRight,
        CopyMotion::PreviousSpace => ViMotion::WordLeft,
        CopyMotion::NextSpaceEnd => ViMotion::WordRightEnd,
        CopyMotion::ScreenTop => ViMotion::High,
        CopyMotion::ScreenMiddle => ViMotion::Middle,
        CopyMotion::ScreenBottom => ViMotion::Low,
        CopyMotion::PreviousParagraph => ViMotion::ParagraphUp,
        CopyMotion::NextParagraph => ViMotion::ParagraphDown,
        CopyMotion::MatchingBracket => ViMotion::Bracket,
    }
}

fn selection_type(selection: CopySelection) -> SelectionType {
    match selection {
        CopySelection::Simple => SelectionType::Simple,
        CopySelection::Lines => SelectionType::Lines,
        CopySelection::Rect => SelectionType::Block,
    }
}

#[expect(
    dead_code,
    reason = "wired again when local copy-mode search ships; kept so the conversion is ready for CopyModeAction::Prompt's reconnection"
)]
fn prompt_kind(dir: CopyPromptDir) -> PromptKind {
    match dir {
        CopyPromptDir::SearchForward => PromptKind::SearchForward,
        CopyPromptDir::SearchBackward => PromptKind::SearchBackward,
        CopyPromptDir::JumpForward => PromptKind::JumpForward,
        CopyPromptDir::JumpBackward => PromptKind::JumpBackward,
        CopyPromptDir::JumpToForward => PromptKind::JumpToForward,
        CopyPromptDir::JumpToBackward => PromptKind::JumpToBackward,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::vi::{ViPromptRequest, ViSearchStepRequest};
    use ozmux_configs::copy_mode::{CopyModeConfig, CopyScroll, CopySearchStep};

    fn resolved_default() -> ResolvedCopyModeKeys {
        let cfg = CopyModeConfig::default();
        let mut map = HashMap::new();
        for (_, keys, action) in cfg.bindings_iter() {
            for key in keys {
                map.insert(normalize(key).unwrap(), action);
            }
        }
        ResolvedCopyModeKeys(map)
    }

    fn none_mods() -> Modifiers {
        Modifiers::default()
    }

    fn ctrl_mods() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    #[test]
    fn resolves_case_sensitive_chars() {
        let r = resolved_default();
        assert_eq!(
            r.resolve(&Key::Character("v".into()), KeyCode::KeyV, none_mods()),
            Some(CopyModeAction::Selection(CopySelection::Simple))
        );
        assert_eq!(
            r.resolve(
                &Key::Character("V".into()),
                KeyCode::KeyV,
                Modifiers {
                    shift: true,
                    ..Default::default()
                }
            ),
            Some(CopyModeAction::Selection(CopySelection::Lines))
        );
        assert_eq!(
            r.resolve(
                &Key::Character("$".into()),
                KeyCode::Digit4,
                Modifiers {
                    shift: true,
                    ..Default::default()
                }
            ),
            Some(CopyModeAction::Motion(CopyMotion::LineEnd))
        );
    }

    #[test]
    fn resolves_ctrl_on_physical_key_and_named_keys() {
        let r = resolved_default();
        assert_eq!(
            r.resolve(&Key::Character("f".into()), KeyCode::KeyF, ctrl_mods()),
            Some(CopyModeAction::Scroll(CopyScroll::PageDown))
        );
        assert_eq!(
            r.resolve(&Key::Escape, KeyCode::Escape, none_mods()),
            Some(CopyModeAction::Exit)
        );
        assert_eq!(
            r.resolve(&Key::Enter, KeyCode::Enter, none_mods()),
            Some(CopyModeAction::Yank)
        );
    }

    #[test]
    fn meta_alt_and_unbound_resolve_to_none() {
        let r = resolved_default();
        assert_eq!(
            r.resolve(
                &Key::Character("q".into()),
                KeyCode::KeyQ,
                Modifiers {
                    meta: true,
                    ..Default::default()
                }
            ),
            None
        );
        assert_eq!(
            r.resolve(&Key::Character(":".into()), KeyCode::Semicolon, none_mods()),
            None
        );
    }

    #[test]
    fn search_step_direction_maps_to_forward_flag() {
        let r = resolved_default();
        assert_eq!(
            r.resolve(&Key::Character("n".into()), KeyCode::KeyN, none_mods()),
            Some(CopyModeAction::SearchStep(CopySearchStep::Next))
        );
        assert_eq!(
            r.resolve(
                &Key::Character("N".into()),
                KeyCode::KeyN,
                Modifiers {
                    shift: true,
                    ..Default::default()
                }
            ),
            Some(CopyModeAction::SearchStep(CopySearchStep::Previous))
        );
    }

    #[test]
    fn prompt_action_is_inert_for_v1() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_configs::copy_mode::{CopyModeAction, CopyPromptDir};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let fired = Arc::new(AtomicBool::new(false));
        let fired_write = fired.clone();
        app.add_observer(move |_ev: On<ViPromptRequest>| {
            fired_write.store(true, Ordering::SeqCst);
        });
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                trigger_copy_mode_action(
                    &mut commands,
                    entity,
                    CopyModeAction::Prompt(CopyPromptDir::SearchForward),
                );
            })
            .unwrap();
        app.update();
        assert!(
            !fired.load(Ordering::SeqCst),
            "Prompt action must be inert in v1"
        );
    }

    #[test]
    fn search_step_action_is_inert_for_v1() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_configs::copy_mode::{CopyModeAction, CopySearchStep};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let fired = Arc::new(AtomicBool::new(false));
        let fired_write = fired.clone();
        app.add_observer(move |_ev: On<ViSearchStepRequest>| {
            fired_write.store(true, Ordering::SeqCst);
        });
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                trigger_copy_mode_action(
                    &mut commands,
                    entity,
                    CopyModeAction::SearchStep(CopySearchStep::Next),
                );
            })
            .unwrap();
        app.update();
        assert!(
            !fired.load(Ordering::SeqCst),
            "SearchStep action must be inert in v1"
        );
    }
}
