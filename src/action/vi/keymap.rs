//! Startup-resolved `[vi-mode]` key table: normalizes each configured key
//! into a lookup form (`Plain` logical char / `Named` / `Ctrl` physical key)
//! and maps a matched action to the shared VI events. Used by both modes'
//! vi-mode key gathers.

use crate::action::vi::{
    ViExitRequest, ViMotionRequest, ViScrollRequest, ViSelectionToggleRequest, ViYankRequest,
};
use crate::configs::OrzmaConfigsResource;
use bevy::input::keyboard::{Key, KeyCode};
use bevy::prelude::*;
use orzma_configs::shortcuts::Modifiers;
use orzma_configs::vi_mode::{
    ViModeAction, ViModeBaseKey, ViModeKey, ViModeMotion, ViModeNamedKey, ViModePromptDir,
    ViModeSelection,
};
use orzma_tmux::PromptKind;
use orzma_tty_engine::{SelectionType, ViMotion};
use std::collections::HashMap;

/// Registers the `Startup` resolution of the `[vi-mode]` table.
pub(super) struct ViModeKeymapPlugin;

impl Plugin for ViModeKeymapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ResolvedViModeKeys>()
            .add_systems(Startup, build_vi_mode_keys);
    }
}

/// The `[vi-mode]` table resolved to a lookup map. Built once at startup.
#[derive(Resource, Default, Debug)]
pub(crate) struct ResolvedViModeKeys(HashMap<ResolvedKey, ViModeAction>);

impl ResolvedViModeKeys {
    /// Returns the action bound to one keypress, or `None` (unbound —
    /// swallowed by vi mode). `Cmd`/`Alt` chords are never vi-mode keys.
    pub(crate) fn resolve(
        &self,
        logical_key: &Key,
        key_code: KeyCode,
        mods: Modifiers,
    ) -> Option<ViModeAction> {
        if mods.meta || mods.alt {
            return None;
        }
        if mods.ctrl {
            // NOTE: the Ctrl table keys on the physical KeyCode, which cannot
            // carry Shift — without this guard Ctrl+Shift+D would silently
            // alias to the Ctrl+D binding (scroll half a page down).
            if mods.shift {
                return None;
            }
            return self.0.get(&ResolvedKey::Ctrl(key_code)).copied();
        }
        let key = match logical_key {
            Key::Character(s) => ResolvedKey::Plain(s.to_string()),
            Key::Escape => ResolvedKey::Named(ViModeNamedKey::Escape),
            Key::Enter => ResolvedKey::Named(ViModeNamedKey::Enter),
            Key::Space => ResolvedKey::Named(ViModeNamedKey::Space),
            Key::Tab => ResolvedKey::Named(ViModeNamedKey::Tab),
            Key::Backspace => ResolvedKey::Named(ViModeNamedKey::Backspace),
            Key::ArrowUp => ResolvedKey::Named(ViModeNamedKey::ArrowUp),
            Key::ArrowDown => ResolvedKey::Named(ViModeNamedKey::ArrowDown),
            Key::ArrowLeft => ResolvedKey::Named(ViModeNamedKey::ArrowLeft),
            Key::ArrowRight => ResolvedKey::Named(ViModeNamedKey::ArrowRight),
            _ => return None,
        };
        self.0.get(&key).copied()
    }
}

/// Fires the VI event for a matched action on `entity`, converting the
/// config-crate vocabulary to engine types. Shared by both modes' gathers.
pub(crate) fn trigger_vi_mode_action(
    commands: &mut Commands,
    entity: Entity,
    action: ViModeAction,
) {
    match action {
        ViModeAction::Motion(m) => commands.trigger(ViMotionRequest {
            entity,
            motion: vi_motion(m),
        }),
        ViModeAction::Scroll(s) => commands.trigger(ViScrollRequest { entity, kind: s }),
        ViModeAction::Selection(s) => commands.trigger(ViSelectionToggleRequest {
            entity,
            ty: selection_type(s),
        }),
        ViModeAction::Yank => commands.trigger(ViYankRequest { entity }),
        ViModeAction::Exit => commands.trigger(ViExitRequest { entity }),
        // TODO: wire to the local search applier once it exists (v1 defers
        // local vi-mode search); until then these actions are swallowed.
        ViModeAction::Prompt(_) => {}
        ViModeAction::SearchStep(_) => {}
    }
}

/// A configured key normalized for runtime lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ResolvedKey {
    /// Exact logical character (case-sensitive).
    Plain(String),
    /// A named logical key.
    Named(ViModeNamedKey),
    /// `Ctrl+` entry, matched on the physical key.
    Ctrl(KeyCode),
}

/// `Startup` system: resolves `[vi-mode]` into the lookup map. Config
/// validation already rejected duplicates, so plain inserts are safe.
fn build_vi_mode_keys(
    mut resolved: ResMut<ResolvedViModeKeys>,
    configs: Res<OrzmaConfigsResource>,
) {
    let mut map = HashMap::new();
    for (label, keys, action) in configs.vi_mode.bindings_iter() {
        for key in keys {
            match normalize(key) {
                Some(rk) => {
                    map.insert(rk, action);
                }
                None => tracing::warn!(
                    label,
                    key = %key,
                    "vi-mode key has no physical mapping; ignoring binding"
                ),
            }
        }
    }
    resolved.0 = map;
}

/// Normalizes a config key to its lookup form. `None` only for `Ctrl+` chars
/// outside the a-z/0-9 physical map (config validation already restricts to
/// ASCII alphanumerics, so this is defensive).
fn normalize(key: &ViModeKey) -> Option<ResolvedKey> {
    match (&key.key, key.ctrl) {
        (ViModeBaseKey::Char(c), false) => Some(ResolvedKey::Plain(c.clone())),
        (ViModeBaseKey::Named(n), false) => Some(ResolvedKey::Named(*n)),
        (ViModeBaseKey::Char(c), true) => {
            ctrl_char_keycode(c.chars().next()?).map(ResolvedKey::Ctrl)
        }
        (ViModeBaseKey::Named(n), true) => Some(ResolvedKey::Ctrl(named_keycode(*n))),
    }
}

fn named_keycode(named: ViModeNamedKey) -> KeyCode {
    match named {
        ViModeNamedKey::Escape => KeyCode::Escape,
        ViModeNamedKey::Enter => KeyCode::Enter,
        ViModeNamedKey::Space => KeyCode::Space,
        ViModeNamedKey::Tab => KeyCode::Tab,
        ViModeNamedKey::Backspace => KeyCode::Backspace,
        ViModeNamedKey::ArrowUp => KeyCode::ArrowUp,
        ViModeNamedKey::ArrowDown => KeyCode::ArrowDown,
        ViModeNamedKey::ArrowLeft => KeyCode::ArrowLeft,
        ViModeNamedKey::ArrowRight => KeyCode::ArrowRight,
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

fn vi_motion(motion: ViModeMotion) -> ViMotion {
    match motion {
        ViModeMotion::Left => ViMotion::Left,
        ViModeMotion::Down => ViMotion::Down,
        ViModeMotion::Up => ViMotion::Up,
        ViModeMotion::Right => ViMotion::Right,
        ViModeMotion::LineStart => ViMotion::First,
        ViModeMotion::LineEnd => ViMotion::Last,
        ViModeMotion::LineFirstChar => ViMotion::FirstOccupied,
        ViModeMotion::NextWord => ViMotion::SemanticRight,
        ViModeMotion::PreviousWord => ViMotion::SemanticLeft,
        ViModeMotion::NextWordEnd => ViMotion::SemanticRightEnd,
        ViModeMotion::NextSpace => ViMotion::WordRight,
        ViModeMotion::PreviousSpace => ViMotion::WordLeft,
        ViModeMotion::NextSpaceEnd => ViMotion::WordRightEnd,
        ViModeMotion::ScreenTop => ViMotion::High,
        ViModeMotion::ScreenMiddle => ViMotion::Middle,
        ViModeMotion::ScreenBottom => ViMotion::Low,
        ViModeMotion::PreviousParagraph => ViMotion::ParagraphUp,
        ViModeMotion::NextParagraph => ViMotion::ParagraphDown,
        ViModeMotion::MatchingBracket => ViMotion::Bracket,
    }
}

fn selection_type(selection: ViModeSelection) -> SelectionType {
    match selection {
        ViModeSelection::Simple => SelectionType::Simple,
        ViModeSelection::Lines => SelectionType::Lines,
        ViModeSelection::Rect => SelectionType::Block,
    }
}

#[expect(
    dead_code,
    reason = "wired again when local vi-mode search ships; kept so the conversion is ready for ViModeAction::Prompt's reconnection"
)]
fn prompt_kind(dir: ViModePromptDir) -> PromptKind {
    match dir {
        ViModePromptDir::SearchForward => PromptKind::SearchForward,
        ViModePromptDir::SearchBackward => PromptKind::SearchBackward,
        ViModePromptDir::JumpForward => PromptKind::JumpForward,
        ViModePromptDir::JumpBackward => PromptKind::JumpBackward,
        ViModePromptDir::JumpToForward => PromptKind::JumpToForward,
        ViModePromptDir::JumpToBackward => PromptKind::JumpToBackward,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::vi::{ViPromptRequest, ViSearchStepRequest};
    use orzma_configs::vi_mode::{ViModeConfig, ViModeScroll, ViModeSearchStep};

    fn resolved_default() -> ResolvedViModeKeys {
        let cfg = ViModeConfig::default();
        let mut map = HashMap::new();
        for (_, keys, action) in cfg.bindings_iter() {
            for key in keys {
                map.insert(normalize(key).unwrap(), action);
            }
        }
        ResolvedViModeKeys(map)
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
            Some(ViModeAction::Selection(ViModeSelection::Simple))
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
            Some(ViModeAction::Selection(ViModeSelection::Lines))
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
            Some(ViModeAction::Motion(ViModeMotion::LineEnd))
        );
    }

    #[test]
    fn resolves_ctrl_on_physical_key_and_named_keys() {
        let r = resolved_default();
        assert_eq!(
            r.resolve(&Key::Character("f".into()), KeyCode::KeyF, ctrl_mods()),
            Some(ViModeAction::Scroll(ViModeScroll::PageDown))
        );
        assert_eq!(
            r.resolve(&Key::Escape, KeyCode::Escape, none_mods()),
            Some(ViModeAction::Exit)
        );
        assert_eq!(
            r.resolve(&Key::Enter, KeyCode::Enter, none_mods()),
            Some(ViModeAction::Yank)
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
            Some(ViModeAction::SearchStep(ViModeSearchStep::Next))
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
            Some(ViModeAction::SearchStep(ViModeSearchStep::Previous))
        );
    }

    #[test]
    fn prompt_action_is_inert_for_v1() {
        use bevy::ecs::system::RunSystemOnce;
        use orzma_configs::vi_mode::{ViModeAction, ViModePromptDir};
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
                trigger_vi_mode_action(
                    &mut commands,
                    entity,
                    ViModeAction::Prompt(ViModePromptDir::SearchForward),
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
        use orzma_configs::vi_mode::{ViModeAction, ViModeSearchStep};
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
                trigger_vi_mode_action(
                    &mut commands,
                    entity,
                    ViModeAction::SearchStep(ViModeSearchStep::Next),
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
