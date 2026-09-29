//! Per-key modifier state for keyboard dispatch: which held Alt / Option key
//! counts as Alt for a given key.

use crate::configs::OrzmaConfigsResource;
use crate::input::current_modifiers;
use bevy::input::keyboard::{Key, KeyCode};
use bevy::prelude::*;
use orzma_configs::keyboard::OptionAsAlt;
use orzma_configs::shortcuts::Modifiers;

/// Resolves [`AltPolicy`] from the loaded configuration at startup.
pub(super) struct HeldModifiersPlugin;

impl Plugin for HeldModifiersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AltPolicy>()
            .add_systems(Startup, build_alt_policy);
    }
}

/// The modifier keys held this frame, with the left and right Alt / Option
/// keys kept apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HeldModifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub meta: bool,
    pub alt_left: bool,
    pub alt_right: bool,
    /// Whether the keyboard layout reports the held right Alt as AltGr.
    pub alt_graph: bool,
}

impl HeldModifiers {
    /// Reads the held modifiers from the physical and the logical key state.
    pub fn from_input(keys: &ButtonInput<KeyCode>, logical: &ButtonInput<Key>) -> Self {
        let shared = current_modifiers(keys);
        Self {
            ctrl: shared.ctrl,
            shift: shared.shift,
            meta: shared.meta,
            alt_left: keys.pressed(KeyCode::AltLeft),
            alt_right: keys.pressed(KeyCode::AltRight),
            alt_graph: logical.pressed(Key::AltGraph),
        }
    }

    /// Returns the held modifiers with every held Alt key counted, AltGr
    /// included.
    pub fn raw(&self) -> Modifiers {
        Modifiers {
            ctrl: self.ctrl,
            shift: self.shift,
            alt: self.alt_left || self.alt_right,
            meta: self.meta,
        }
    }

    /// Returns the modifiers `logical` is dispatched with under `policy`.
    ///
    /// A held right Alt that the layout reports as AltGr never counts as Alt.
    /// Any other held Alt / Option key counts when `policy` makes its side
    /// always Alt, or when the key composes no character: `logical` is not a
    /// [`Key::Character`], or Ctrl or Cmd is held with it.
    pub fn for_key(&self, logical: &Key, policy: AltPolicy) -> Modifiers {
        let composes = self.composes_character(logical);
        Modifiers {
            ctrl: self.ctrl,
            shift: self.shift,
            alt: self.left_counts_as_alt(policy, composes)
                || self.right_counts_as_alt(policy, composes),
            meta: self.meta,
        }
    }

    /// Whether a held Option key composes `logical` into a character:
    /// `logical` is a [`Key::Character`] and neither Ctrl nor Cmd is held.
    fn composes_character(&self, logical: &Key) -> bool {
        matches!(logical, Key::Character(_)) && !self.ctrl && !self.meta
    }

    /// Whether the left Alt / Option key is held and counts as Alt: `policy`
    /// makes the left side always Alt, or the key composes no character.
    fn left_counts_as_alt(&self, policy: AltPolicy, composes: bool) -> bool {
        self.alt_left && (policy.left_always_alt || !composes)
    }

    /// Whether the right Alt / Option key is held and counts as Alt: it is not
    /// AltGr, and `policy` makes the right side always Alt or the key composes
    /// no character.
    fn right_counts_as_alt(&self, policy: AltPolicy, composes: bool) -> bool {
        self.alt_right && !self.alt_graph && (policy.right_always_alt || !composes)
    }
}

#[cfg(test)]
impl From<Modifiers> for HeldModifiers {
    fn from(mods: Modifiers) -> Self {
        Self {
            ctrl: mods.ctrl,
            shift: mods.shift,
            meta: mods.meta,
            alt_left: mods.alt,
            alt_right: false,
            alt_graph: false,
        }
    }
}

/// Which held Alt / Option keys count as Alt even for a key that composes a
/// character.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AltPolicy {
    pub left_always_alt: bool,
    pub right_always_alt: bool,
}

impl Default for AltPolicy {
    /// Counts both sides as Alt.
    fn default() -> Self {
        Self {
            left_always_alt: true,
            right_always_alt: true,
        }
    }
}

impl AltPolicy {
    /// Returns the policy for the platform this build targets: the
    /// `option_as_alt` sides on macOS, and both sides elsewhere.
    pub fn for_host(option_as_alt: OptionAsAlt) -> Self {
        if cfg!(target_os = "macos") {
            Self::for_option_as_alt(option_as_alt)
        } else {
            Self::default()
        }
    }

    /// Returns the policy that makes exactly the Option keys `option_as_alt`
    /// names always Alt.
    pub fn for_option_as_alt(option_as_alt: OptionAsAlt) -> Self {
        let (left_always_alt, right_always_alt) = match option_as_alt {
            OptionAsAlt::None => (false, false),
            OptionAsAlt::Left => (true, false),
            OptionAsAlt::Right => (false, true),
            OptionAsAlt::Both => (true, true),
        };
        Self {
            left_always_alt,
            right_always_alt,
        }
    }
}

/// Replaces the default [`AltPolicy`] with the one the configuration selects.
fn build_alt_policy(mut policy: ResMut<AltPolicy>, configs: Res<OrzmaConfigsResource>) {
    let resolved = AltPolicy::for_host(configs.keyboard.option_as_alt);
    if *policy != resolved {
        *policy = resolved;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_configs::OrzmaConfigs;
    use orzma_configs::keyboard::KeyboardConfig;

    fn character(text: &str) -> Key {
        Key::Character(text.into())
    }

    /// Asserts that a right Alt the layout reports as AltGr never counts as
    /// Alt, for a character or a named key.
    ///
    /// Case: a user on a German Windows layout presses AltGr+7 to type `{`, or
    /// holds AltGr while pressing an arrow key.
    #[test]
    fn altgr_never_counts_as_alt() {
        let held = HeldModifiers {
            alt_right: true,
            alt_graph: true,
            ..Default::default()
        };
        assert!(!held.for_key(&character("{"), AltPolicy::default()).alt);
        assert!(!held.for_key(&Key::ArrowLeft, AltPolicy::default()).alt);
    }

    /// Asserts that under the default policy the left Alt and a right Alt that
    /// is not AltGr both count as Alt for a character key.
    ///
    /// Case: a user on a US Windows layout presses left Alt+h or right Alt+h.
    #[test]
    fn plain_alt_keys_count_under_the_default_policy() {
        let left = HeldModifiers {
            alt_left: true,
            ..Default::default()
        };
        let right = HeldModifiers {
            alt_right: true,
            ..Default::default()
        };
        assert!(left.for_key(&character("h"), AltPolicy::default()).alt);
        assert!(right.for_key(&character("h"), AltPolicy::default()).alt);
    }

    /// Asserts that an Option key that is not always Alt does not count as
    /// Alt when it composes a character, Shift included.
    ///
    /// Case: a macOS user with `option_as_alt = "right"` types `˙` with left
    /// Option+h and `Ó` with left Option+Shift+H.
    #[test]
    fn a_composing_option_key_does_not_count_for_a_character() {
        let policy = AltPolicy::for_option_as_alt(OptionAsAlt::Right);
        let left = HeldModifiers {
            alt_left: true,
            ..Default::default()
        };
        let left_shift = HeldModifiers {
            shift: true,
            ..left
        };
        assert_eq!(left.for_key(&character("˙"), policy), Modifiers::default());
        assert!(!left_shift.for_key(&character("Ó"), policy).alt);
        assert!(left_shift.for_key(&character("Ó"), policy).shift);
    }

    /// Asserts that the Option key `option_as_alt` names counts as Alt for a
    /// character key, Shift included.
    ///
    /// Case: a macOS user with `option_as_alt = "right"` presses right
    /// Option+h to select a pane and right Option+Shift+H to resize it.
    #[test]
    fn the_alt_option_key_counts_for_a_character() {
        let policy = AltPolicy::for_option_as_alt(OptionAsAlt::Right);
        let right = HeldModifiers {
            alt_right: true,
            ..Default::default()
        };
        let right_shift = HeldModifiers {
            shift: true,
            ..right
        };
        assert!(right.for_key(&character("h"), policy).alt);
        let resized = right_shift.for_key(&character("H"), policy);
        assert!(resized.alt && resized.shift);
    }

    /// Asserts that a composing Option key counts as Alt for a named key.
    ///
    /// Case: a macOS user with `option_as_alt = "right"` presses left
    /// Option+ArrowLeft, which types no character.
    #[test]
    fn a_composing_option_key_counts_for_a_named_key() {
        let policy = AltPolicy::for_option_as_alt(OptionAsAlt::Right);
        let left = HeldModifiers {
            alt_left: true,
            ..Default::default()
        };
        assert!(left.for_key(&Key::ArrowLeft, policy).alt);
    }

    /// Asserts that a composing Option key counts as Alt when Ctrl or Cmd is
    /// held with it.
    ///
    /// Case: a macOS user with `option_as_alt = "right"` presses left
    /// Option+Ctrl+Q or left Option+Cmd+K, where the Option key types no
    /// symbol.
    #[test]
    fn a_composing_option_key_counts_with_ctrl_or_cmd() {
        let policy = AltPolicy::for_option_as_alt(OptionAsAlt::Right);
        let with_ctrl = HeldModifiers {
            ctrl: true,
            alt_left: true,
            ..Default::default()
        };
        let with_cmd = HeldModifiers {
            meta: true,
            alt_left: true,
            ..Default::default()
        };
        assert!(with_ctrl.for_key(&character("q"), policy).alt);
        assert!(with_cmd.for_key(&character("k"), policy).alt);
    }

    /// Asserts that under `none` neither Option key counts as Alt for a
    /// character, and under `both` either one does.
    ///
    /// Case: a macOS user types `¬` with right Option+l under `none`, then
    /// switches to `both` and selects a pane with left Option+l.
    #[test]
    fn none_and_both_decide_every_option_key() {
        let right = HeldModifiers {
            alt_right: true,
            ..Default::default()
        };
        let left = HeldModifiers {
            alt_left: true,
            ..Default::default()
        };
        let none = AltPolicy::for_option_as_alt(OptionAsAlt::None);
        let both = AltPolicy::for_option_as_alt(OptionAsAlt::Both);
        assert!(!right.for_key(&character("¬"), none).alt);
        assert!(left.for_key(&character("l"), both).alt);
    }

    /// Asserts that each `option_as_alt` value makes exactly the Option keys it
    /// names always Alt.
    ///
    /// Case: a macOS user sets `option_as_alt` to each of its four values.
    #[test]
    fn option_as_alt_selects_the_always_alt_sides() {
        let sides = |mode| {
            let p = AltPolicy::for_option_as_alt(mode);
            (p.left_always_alt, p.right_always_alt)
        };
        assert_eq!(sides(OptionAsAlt::None), (false, false));
        assert_eq!(sides(OptionAsAlt::Left), (true, false));
        assert_eq!(sides(OptionAsAlt::Right), (false, true));
        assert_eq!(sides(OptionAsAlt::Both), (true, true));
    }

    /// Asserts that `raw` counts every held Alt key, AltGr included.
    ///
    /// Case: a focused webview needs the modifiers exactly as the keyboard
    /// reports them while the user holds AltGr.
    #[test]
    fn raw_counts_every_held_alt() {
        let held = HeldModifiers {
            alt_right: true,
            alt_graph: true,
            ..Default::default()
        };
        assert!(held.raw().alt);
    }

    /// Asserts that `from_input` reads each side and AltGr from the key state.
    ///
    /// Case: a user holds left Ctrl and AltGr on a German layout.
    #[test]
    fn from_input_reads_sides_and_altgr() {
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::AltRight);
        let mut logical = ButtonInput::<Key>::default();
        logical.press(Key::AltGraph);
        let held = HeldModifiers::from_input(&keys, &logical);
        assert_eq!(
            held,
            HeldModifiers {
                ctrl: true,
                alt_right: true,
                alt_graph: true,
                ..Default::default()
            }
        );
    }

    /// Asserts that the startup system installs the policy the configuration
    /// selects for this platform.
    ///
    /// Case: a user sets `option_as_alt = "left"` and starts orzma.
    #[test]
    fn build_alt_policy_reads_the_config() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(HeldModifiersPlugin)
            .insert_resource(OrzmaConfigsResource(OrzmaConfigs {
                keyboard: KeyboardConfig {
                    option_as_alt: OptionAsAlt::Left,
                },
                ..Default::default()
            }));
        app.update();
        assert_eq!(
            *app.world().resource::<AltPolicy>(),
            AltPolicy::for_host(OptionAsAlt::Left)
        );
    }
}
