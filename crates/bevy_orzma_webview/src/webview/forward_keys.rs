//! Forward-key chords: the wire chords a registration names, converted to the
//! host's key types and kept current on every mounted webview of the
//! registration.

use crate::webview::mount::Webview;
use bevy::prelude::*;
use bevy_orzmux::prelude::OrzmuxWebviewEvent;
use orzma_webview_host::prelude::{ForwardChord, WebviewEvent};

/// The key a forward-key chord matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordKey {
    /// A physical key, compared together with the exact modifier set.
    Code(KeyCode),
    /// A printable ASCII punctuation character, compared against the
    /// character the key produced, with Shift ignored.
    Char(char),
}

/// A forward-key chord normalized to host input types: the key it matches
/// plus modifier booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedChord {
    /// The key the chord matches.
    pub key: ChordKey,
    /// Alt modifier active.
    pub alt: bool,
    /// Ctrl modifier active.
    pub ctrl: bool,
    /// Shift modifier active.
    pub shift: bool,
    /// The Super/Command/Meta modifier (bevy calls it Super/logo).
    pub logo: bool,
}

/// The forward-key chords of a mounted webview: key presses that reach the
/// pane's PTY instead of the focused page.
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
pub struct ForwardKeys(pub Vec<NormalizedChord>);

/// Keeps every mounted webview's `ForwardKeys` in step with the host's
/// `ForwardKeysChanged` events.
pub(crate) struct ForwardKeysPlugin;

impl Plugin for ForwardKeysPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_forward_keys_changed);
    }
}

impl NormalizedChord {
    /// Normalizes a wire chord, returning `None` for an unrecognized key
    /// name.
    ///
    /// `"backtab"` maps to the same physical key as `"tab"`; the Shift
    /// distinction rides the modifier bits. A name made of one ASCII
    /// punctuation character maps to [`ChordKey::Char`]. Unknown modifier
    /// names are ignored.
    pub(crate) fn parse(chord: &ForwardChord) -> Option<Self> {
        let key = ChordKey::from_name(chord.key())?;
        let mut normalized = Self {
            key,
            alt: false,
            ctrl: false,
            shift: false,
            logo: false,
        };
        for m in chord.mods() {
            match m.as_str() {
                "alt" => normalized.alt = true,
                "ctrl" => normalized.ctrl = true,
                "shift" => normalized.shift = true,
                "meta" => normalized.logo = true,
                _ => {}
            }
        }
        Some(normalized)
    }
}

impl ChordKey {
    /// Maps a wire key name to the key it matches; `None` when unrecognized.
    fn from_name(name: &str) -> Option<Self> {
        let code = match name {
            "tab" | "backtab" => KeyCode::Tab,
            "f1" => KeyCode::F1,
            "f2" => KeyCode::F2,
            "f3" => KeyCode::F3,
            "f4" => KeyCode::F4,
            "f5" => KeyCode::F5,
            "f6" => KeyCode::F6,
            "f7" => KeyCode::F7,
            "f8" => KeyCode::F8,
            "f9" => KeyCode::F9,
            "f10" => KeyCode::F10,
            "f11" => KeyCode::F11,
            "f12" => KeyCode::F12,
            "0" => KeyCode::Digit0,
            "1" => KeyCode::Digit1,
            "2" => KeyCode::Digit2,
            "3" => KeyCode::Digit3,
            "4" => KeyCode::Digit4,
            "5" => KeyCode::Digit5,
            "6" => KeyCode::Digit6,
            "7" => KeyCode::Digit7,
            "8" => KeyCode::Digit8,
            "9" => KeyCode::Digit9,
            "a" => KeyCode::KeyA,
            "b" => KeyCode::KeyB,
            "c" => KeyCode::KeyC,
            "d" => KeyCode::KeyD,
            "e" => KeyCode::KeyE,
            "f" => KeyCode::KeyF,
            "g" => KeyCode::KeyG,
            "h" => KeyCode::KeyH,
            "i" => KeyCode::KeyI,
            "j" => KeyCode::KeyJ,
            "k" => KeyCode::KeyK,
            "l" => KeyCode::KeyL,
            "m" => KeyCode::KeyM,
            "n" => KeyCode::KeyN,
            "o" => KeyCode::KeyO,
            "p" => KeyCode::KeyP,
            "q" => KeyCode::KeyQ,
            "r" => KeyCode::KeyR,
            "s" => KeyCode::KeyS,
            "t" => KeyCode::KeyT,
            "u" => KeyCode::KeyU,
            "v" => KeyCode::KeyV,
            "w" => KeyCode::KeyW,
            "x" => KeyCode::KeyX,
            "y" => KeyCode::KeyY,
            "z" => KeyCode::KeyZ,
            "esc" => KeyCode::Escape,
            " " => KeyCode::Space,
            "down" => KeyCode::ArrowDown,
            "up" => KeyCode::ArrowUp,
            "left" => KeyCode::ArrowLeft,
            "right" => KeyCode::ArrowRight,
            "pagedown" => KeyCode::PageDown,
            "pageup" => KeyCode::PageUp,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "enter" => KeyCode::Enter,
            "backspace" => KeyCode::Backspace,
            "delete" => KeyCode::Delete,
            _ => return Self::punctuation(name),
        };
        Some(Self::Code(code))
    }

    /// Maps a name made of exactly one ASCII punctuation character to a
    /// character chord.
    fn punctuation(name: &str) -> Option<Self> {
        let mut chars = name.chars();
        let c = chars.next()?;
        (chars.next().is_none() && c.is_ascii_punctuation()).then_some(Self::Char(c))
    }
}

impl ForwardKeys {
    /// The chords among `chords` whose key names are recognized, in order.
    pub fn from_wire(chords: &[ForwardChord]) -> Self {
        Self(chords.iter().filter_map(NormalizedChord::parse).collect())
    }
}

/// Replaces the `ForwardKeys` of every mounted webview of the handle a
/// `ForwardKeysChanged` names.
fn apply_forward_keys_changed(
    ev: On<OrzmuxWebviewEvent>,
    mut commands: Commands,
    webviews: Query<(Entity, &Webview)>,
) {
    let WebviewEvent::ForwardKeysChanged { handle, keys } = ev.webview_event() else {
        return;
    };
    let chords = ForwardKeys::from_wire(keys);
    for (entity, view) in &webviews {
        if view.handle() == handle {
            commands.entity(entity).try_insert(chords.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_vt::prelude::InstanceId;
    use orzma_webview_host::prelude::{HandleId, MountId};
    use orzmux::prelude::CommandSeq;

    fn chord(mods: &[&str], key: &str) -> ForwardChord {
        ForwardChord::new(mods.iter().map(|m| (*m).to_owned()).collect(), key)
    }

    /// Asserts that a letter, a function key and `tab` parse to their
    /// physical keys with the declared modifiers, and an unknown name fails.
    ///
    /// Case: a program registers Alt+h, F5 and Tab as forward keys.
    #[test]
    fn parse_maps_keys_and_mods() {
        let n = NormalizedChord::parse(&chord(&["alt"], "h")).unwrap();
        assert_eq!(n.key, ChordKey::Code(KeyCode::KeyH));
        assert!(n.alt && !n.ctrl && !n.shift && !n.logo);
        assert_eq!(
            NormalizedChord::parse(&chord(&[], "f5")).map(|c| c.key),
            Some(ChordKey::Code(KeyCode::F5))
        );
        assert_eq!(
            NormalizedChord::parse(&chord(&[], "tab")).map(|c| c.key),
            Some(ChordKey::Code(KeyCode::Tab))
        );
        assert!(NormalizedChord::parse(&chord(&[], "nope")).is_none());
    }

    /// Asserts that the navigation key names the forward-key grammar already
    /// accepted still parse to their physical keys.
    ///
    /// Case: a TUI browser forwards Esc, Space and the arrow and page keys.
    #[test]
    fn parse_maps_forward_keys_keys() {
        let cases: &[(&str, KeyCode)] = &[
            ("esc", KeyCode::Escape),
            (" ", KeyCode::Space),
            ("down", KeyCode::ArrowDown),
            ("up", KeyCode::ArrowUp),
            ("pagedown", KeyCode::PageDown),
            ("pageup", KeyCode::PageUp),
        ];
        for (key, expected) in cases {
            assert_eq!(
                NormalizedChord::parse(&chord(&[], key)).map(|c| c.key),
                Some(ChordKey::Code(*expected)),
                "failed for key={key:?}"
            );
        }
    }

    /// Asserts that the editing and navigation key names added for forward
    /// chords parse to their physical keys.
    ///
    /// Case: a markdown viewer forwards Backspace and Enter so its TUI can go
    /// back and confirm a search while the page holds keyboard focus.
    #[test]
    fn parse_maps_editing_and_navigation_key_names() {
        let cases: &[(&str, KeyCode)] = &[
            ("enter", KeyCode::Enter),
            ("backspace", KeyCode::Backspace),
            ("left", KeyCode::ArrowLeft),
            ("right", KeyCode::ArrowRight),
            ("home", KeyCode::Home),
            ("end", KeyCode::End),
            ("delete", KeyCode::Delete),
        ];
        for (name, expected) in cases {
            assert_eq!(
                NormalizedChord::parse(&chord(&[], name)).map(|c| c.key),
                Some(ChordKey::Code(*expected)),
                "failed for key={name:?}"
            );
        }
    }

    /// Asserts that one ASCII punctuation character parses to a character
    /// chord, while a longer or non-punctuation name is rejected.
    ///
    /// Case: a markdown viewer forwards `/` to open its search and `[` / `]`
    /// to jump between headings.
    #[test]
    fn parse_maps_single_punctuation_to_a_character_chord() {
        for c in ['/', '?', '[', ']', ':'] {
            assert_eq!(
                NormalizedChord::parse(&chord(&[], &c.to_string())).map(|n| n.key),
                Some(ChordKey::Char(c)),
                "failed for key={c:?}"
            );
        }
        for name in ["//", "é", "nope"] {
            assert!(
                NormalizedChord::parse(&chord(&[], name)).is_none(),
                "{name:?} must be rejected"
            );
        }
    }

    /// Asserts that converting a registration's chords keeps the recognized
    /// ones in order and skips a name the host does not know.
    ///
    /// Case: a program registers Alt+h, an unknown `hyper` key, and `/` as
    /// forward keys.
    #[test]
    fn forward_keys_keep_only_recognized_chords() {
        let keys =
            ForwardKeys::from_wire(&[chord(&["alt"], "h"), chord(&[], "hyper"), chord(&[], "/")]);
        assert_eq!(
            keys.0.iter().map(|c| c.key).collect::<Vec<_>>(),
            vec![ChordKey::Code(KeyCode::KeyH), ChordKey::Char('/')]
        );
    }

    /// Asserts that a forward-keys change replaces the chords of every
    /// mounted webview of its handle and leaves other handles alone.
    ///
    /// Case: a markdown viewer mounted twice starts forwarding Esc to its TUI
    /// while another program's page keeps its own chords.
    #[test]
    fn a_forward_keys_change_updates_every_mount_of_its_handle() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(ForwardKeysPlugin);
        let spawn = |app: &mut App, handle: &str, mount: u64| {
            app.world_mut()
                .spawn((
                    Webview::new(
                        HandleId::from(handle),
                        InstanceId(u128::from(mount)),
                        MountId::new(mount),
                        0,
                        10,
                        40,
                    ),
                    ForwardKeys::default(),
                ))
                .id()
        };
        let first = spawn(&mut app, "md", 1);
        let second = spawn(&mut app, "md", 2);
        let other = spawn(&mut app, "other", 3);
        app.world_mut().trigger(OrzmuxWebviewEvent::new(
            WebviewEvent::ForwardKeysChanged {
                handle: HandleId::from("md"),
                keys: vec![chord(&[], "esc")],
            },
            CommandSeq(0),
        ));
        app.world_mut().flush();
        let esc = ForwardKeys::from_wire(&[chord(&[], "esc")]);
        assert_eq!(app.world().get::<ForwardKeys>(first), Some(&esc));
        assert_eq!(app.world().get::<ForwardKeys>(second), Some(&esc));
        assert_eq!(
            app.world().get::<ForwardKeys>(other),
            Some(&ForwardKeys::default())
        );
    }
}
