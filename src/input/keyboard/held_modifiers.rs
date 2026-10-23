//! Per-key modifier state for keyboard dispatch: which held Alt / Option key
//! counts as Alt for a given key, and which modifier keys each frame starts
//! with.

use crate::configs::OrzmaConfigsResource;
use bevy::input::ButtonState;
use bevy::input::InputSystems;
use bevy::input::keyboard::{Key, KeyCode};
use bevy::prelude::*;
use bevy::winit::RawWinitWindowEvent;
use bevy::winit::converters::{
    convert_element_state, convert_logical_key, convert_physical_key_code,
};
use orzma_configs::keyboard::OptionAsAlt;
use orzma_configs::shortcuts::Modifiers;
use winit::event::WindowEvent;

/// Resolves [`AltPolicy`] from the loaded configuration at startup, and
/// tracks the modifier keys each frame starts with.
pub(super) struct HeldModifiersPlugin;

impl Plugin for HeldModifiersPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<RawWinitWindowEvent>()
            .init_resource::<AltPolicy>()
            .init_resource::<FrameStartModifiers>()
            .add_systems(Startup, build_alt_policy)
            .add_systems(PreUpdate, record_frame_start_modifiers.after(InputSystems));
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
        ModifierKeys::from_input(keys, logical).held()
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
            alt: self.left_counts_as_alt(policy, composes)
                || self.right_counts_as_alt(policy, composes),
            ..self.raw()
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

/// Which modifier keys are held, each side of each modifier kept apart, with
/// the logical AltGr state alongside the physical right Alt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ModifierKeys {
    control_left: bool,
    control_right: bool,
    shift_left: bool,
    shift_right: bool,
    alt_left: bool,
    alt_right: bool,
    super_left: bool,
    super_right: bool,
    alt_graph: bool,
}

impl ModifierKeys {
    /// Reads the held modifier keys from the physical and the logical key
    /// state.
    pub fn from_input(keys: &ButtonInput<KeyCode>, logical: &ButtonInput<Key>) -> Self {
        Self {
            control_left: keys.pressed(KeyCode::ControlLeft),
            control_right: keys.pressed(KeyCode::ControlRight),
            shift_left: keys.pressed(KeyCode::ShiftLeft),
            shift_right: keys.pressed(KeyCode::ShiftRight),
            alt_left: keys.pressed(KeyCode::AltLeft),
            alt_right: keys.pressed(KeyCode::AltRight),
            super_left: keys.pressed(KeyCode::SuperLeft),
            super_right: keys.pressed(KeyCode::SuperRight),
            alt_graph: logical.pressed(Key::AltGraph),
        }
    }

    /// Marks `key_code` held on a press and not held on a release; a
    /// [`Key::AltGraph`] `logical_key` sets the AltGraph flag the same way,
    /// and any other key changes nothing.
    pub fn apply(&mut self, key_code: KeyCode, logical_key: &Key, state: ButtonState) {
        let held = state == ButtonState::Pressed;
        match key_code {
            KeyCode::ControlLeft => self.control_left = held,
            KeyCode::ControlRight => self.control_right = held,
            KeyCode::ShiftLeft => self.shift_left = held,
            KeyCode::ShiftRight => self.shift_right = held,
            KeyCode::AltLeft => self.alt_left = held,
            KeyCode::AltRight => self.alt_right = held,
            KeyCode::SuperLeft => self.super_left = held,
            KeyCode::SuperRight => self.super_right = held,
            _ => {}
        }
        if *logical_key == Key::AltGraph {
            self.alt_graph = held;
        }
    }

    /// Collapses the held keys into the shared modifier state: each of ctrl,
    /// shift and meta is true when either side is held.
    pub fn held(&self) -> HeldModifiers {
        HeldModifiers {
            ctrl: self.control_left || self.control_right,
            shift: self.shift_left || self.shift_right,
            meta: self.super_left || self.super_right,
            alt_left: self.alt_left,
            alt_right: self.alt_right,
            alt_graph: self.alt_graph,
        }
    }

    /// Returns the modifier state a frame that starts at `start` classifies
    /// its keys with: `start` stepped through `delivered`, then corrected with
    /// every stepped flag whose value differs from `end`, so a change Bevy
    /// added without a winit key event counts from the frame start.
    pub fn frame_start(
        start: Self,
        delivered: impl IntoIterator<Item = DeliveredKey>,
        end: Self,
    ) -> Self {
        let mut stepped = start;
        for key in delivered {
            stepped.apply(key.key_code, &key.logical_key, key.state);
        }
        let mut next = start;
        Self::correct(
            &mut next.control_left,
            stepped.control_left,
            end.control_left,
        );
        Self::correct(
            &mut next.control_right,
            stepped.control_right,
            end.control_right,
        );
        Self::correct(&mut next.shift_left, stepped.shift_left, end.shift_left);
        Self::correct(&mut next.shift_right, stepped.shift_right, end.shift_right);
        Self::correct(&mut next.alt_left, stepped.alt_left, end.alt_left);
        Self::correct(&mut next.alt_right, stepped.alt_right, end.alt_right);
        Self::correct(&mut next.super_left, stepped.super_left, end.super_left);
        Self::correct(&mut next.super_right, stepped.super_right, end.super_right);
        Self::correct(&mut next.alt_graph, stepped.alt_graph, end.alt_graph);
        next
    }

    fn correct(slot: &mut bool, stepped: bool, end: bool) {
        if stepped != end {
            *slot = end;
        }
    }
}

#[cfg(test)]
impl From<HeldModifiers> for ModifierKeys {
    fn from(held: HeldModifiers) -> Self {
        Self {
            control_left: held.ctrl,
            shift_left: held.shift,
            super_left: held.meta,
            alt_left: held.alt_left,
            alt_right: held.alt_right,
            alt_graph: held.alt_graph,
            ..Default::default()
        }
    }
}

#[cfg(test)]
impl From<Modifiers> for ModifierKeys {
    fn from(mods: Modifiers) -> Self {
        Self {
            control_left: mods.ctrl,
            shift_left: mods.shift,
            super_left: mods.meta,
            alt_left: mods.alt,
            ..Default::default()
        }
    }
}

/// One winit key event already converted to Bevy keys: the physical key, the
/// logical key, and whether it was pressed or released.
#[derive(Debug, PartialEq)]
pub(crate) struct DeliveredKey {
    key_code: KeyCode,
    logical_key: Key,
    state: ButtonState,
}

impl DeliveredKey {
    /// Builds the delivered form of one winit key event.
    pub fn new(key_code: KeyCode, logical_key: Key, state: ButtonState) -> Self {
        Self {
            key_code,
            logical_key,
            state,
        }
    }

    /// Converts a winit window event into its delivered key, or `None` for
    /// anything but a non-synthetic key event.
    pub fn from_raw(event: &WindowEvent) -> Option<Self> {
        match event {
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => Some(Self::new(
                convert_physical_key_code(event.physical_key),
                convert_logical_key(&event.logical_key),
                convert_element_state(event.state),
            )),
            _ => None,
        }
    }
}

/// The modifier keys the frame's key events start from.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FrameStartModifiers(ModifierKeys);

impl FrameStartModifiers {
    /// Wraps the modifier state a frame starts from.
    pub fn new(keys: ModifierKeys) -> Self {
        Self(keys)
    }

    /// Returns the modifier state the frame starts from.
    pub fn keys(&self) -> ModifierKeys {
        self.0
    }
}

/// Records the modifier state the frame's key events start from: the previous
/// end stepped through winit's key events, corrected with `ButtonInput` for
/// the transitions winit never delivered.
fn record_frame_start_modifiers(
    mut frame_start: ResMut<FrameStartModifiers>,
    mut raw_events: MessageReader<RawWinitWindowEvent>,
    mut previous_end: Local<ModifierKeys>,
    keys: Res<ButtonInput<KeyCode>>,
    logical: Res<ButtonInput<Key>>,
) {
    let end = ModifierKeys::from_input(&keys, &logical);
    let delivered = raw_events
        .read()
        .filter_map(|raw| DeliveredKey::from_raw(&raw.event));
    let next = ModifierKeys::frame_start(*previous_end, delivered, end);
    if next != frame_start.keys() {
        *frame_start = FrameStartModifiers::new(next);
    }
    *previous_end = end;
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
        Self::for_option_as_alt(OptionAsAlt::Both)
    }
}

impl AltPolicy {
    /// Returns the policy for the platform this build targets: the
    /// `option_as_alt` sides on macOS, and both sides elsewhere.
    pub fn for_host(option_as_alt: OptionAsAlt) -> Self {
        Self::for_option_as_alt(if cfg!(target_os = "macos") {
            option_as_alt
        } else {
            OptionAsAlt::Both
        })
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
    use winit::event::WindowEvent;

    fn character(text: &str) -> Key {
        Key::Character(text.into())
    }

    fn keys_with(pressed: &[(KeyCode, Key)]) -> ModifierKeys {
        let mut keys = ModifierKeys::default();
        for (key_code, logical) in pressed {
            keys.apply(*key_code, logical, ButtonState::Pressed);
        }
        keys
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
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<Key>>()
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

    /// Asserts that releasing one of two held Shift keys keeps Shift held
    /// until the other is released.
    ///
    /// Case: the user holds both Shift keys to type, then lifts the left one
    /// while the right one stays down.
    #[test]
    fn releasing_one_shift_key_keeps_shift_held() {
        let mut keys = keys_with(&[
            (KeyCode::ShiftLeft, Key::Shift),
            (KeyCode::ShiftRight, Key::Shift),
        ]);
        keys.apply(KeyCode::ShiftLeft, &Key::Shift, ButtonState::Released);
        assert!(keys.held().shift);
        keys.apply(KeyCode::ShiftRight, &Key::Shift, ButtonState::Released);
        assert!(!keys.held().shift);
    }

    /// Asserts that pressing an already held modifier key keeps it held.
    ///
    /// Case: the keyboard auto-repeats a Shift press the user keeps holding.
    #[test]
    fn a_repeated_modifier_press_keeps_it_held() {
        let mut keys = keys_with(&[
            (KeyCode::ShiftLeft, Key::Shift),
            (KeyCode::ShiftLeft, Key::Shift),
        ]);
        assert!(keys.held().shift);
        keys.apply(KeyCode::ShiftLeft, &Key::Shift, ButtonState::Released);
        assert_eq!(keys, ModifierKeys::default());
    }

    /// Asserts that lock keys and ordinary keys leave the modifier state
    /// empty.
    ///
    /// Case: the user presses Caps Lock, Num Lock, and the A key while no
    /// modifier is held.
    #[test]
    fn lock_and_other_keys_leave_the_modifiers_alone() {
        let keys = keys_with(&[
            (KeyCode::CapsLock, Key::CapsLock),
            (KeyCode::NumLock, Key::NumLock),
            (KeyCode::KeyA, character("a")),
        ]);
        assert_eq!(keys, ModifierKeys::default());
    }

    /// Asserts that an AltGr press holds right Alt and AltGraph, and that its
    /// release clears both.
    ///
    /// Case: a user on a German Windows layout holds AltGr to type `{`.
    #[test]
    fn altgr_press_and_release_set_alt_right_and_alt_graph() {
        let mut keys = keys_with(&[(KeyCode::AltRight, Key::AltGraph)]);
        assert_eq!(
            keys.held(),
            HeldModifiers {
                alt_right: true,
                alt_graph: true,
                ..Default::default()
            }
        );
        keys.apply(KeyCode::AltRight, &Key::AltGraph, ButtonState::Released);
        assert_eq!(keys, ModifierKeys::default());
    }

    /// Asserts that `from_input` reads each held modifier key, including
    /// AltGr from the logical key state.
    ///
    /// Case: the user holds right Shift, left Super, and AltGr at once.
    #[test]
    fn from_input_reads_each_modifier_key() {
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ShiftRight);
        keys.press(KeyCode::SuperLeft);
        let mut logical = ButtonInput::<Key>::default();
        logical.press(Key::AltGraph);
        assert_eq!(
            ModifierKeys::from_input(&keys, &logical).held(),
            HeldModifiers {
                shift: true,
                meta: true,
                alt_graph: true,
                ..Default::default()
            }
        );
    }

    /// Asserts that modifiers the OS took without a key event count as
    /// released from the frame start.
    ///
    /// Case: the user releases the keys of a Cmd+Shift+5 screenshot chord in
    /// another app, then types `l` back in orzma.
    #[test]
    fn a_release_the_os_took_counts_from_the_frame_start() {
        let start = keys_with(&[
            (KeyCode::SuperLeft, Key::Super),
            (KeyCode::ShiftLeft, Key::Shift),
        ]);
        let delivered = [DeliveredKey::new(
            KeyCode::KeyL,
            character("l"),
            ButtonState::Pressed,
        )];
        assert_eq!(
            ModifierKeys::frame_start(start, delivered, ModifierKeys::default()),
            ModifierKeys::default()
        );
    }

    /// Asserts that a modifier Bevy added without a winit key event counts
    /// as held from the frame start.
    ///
    /// Case: orzma becomes active while the user holds Cmd from Cmd+H in
    /// another app, then presses T.
    #[test]
    fn a_modifier_held_before_activation_counts_from_the_frame_start() {
        let delivered = [DeliveredKey::new(
            KeyCode::KeyT,
            character("t"),
            ButtonState::Pressed,
        )];
        let end = keys_with(&[(KeyCode::SuperLeft, Key::Super)]);
        assert_eq!(
            ModifierKeys::frame_start(ModifierKeys::default(), delivered, end),
            end
        );
    }

    /// Asserts that a delivered modifier release keeps the frame-start state,
    /// since the pressed key arrived while the modifier was held.
    ///
    /// Case: the user presses Enter and releases Shift in the same frame
    /// while typing quickly.
    #[test]
    fn a_delivered_release_keeps_the_frame_start() {
        let start = keys_with(&[(KeyCode::ShiftLeft, Key::Shift)]);
        let delivered = [
            DeliveredKey::new(KeyCode::Enter, Key::Enter, ButtonState::Pressed),
            DeliveredKey::new(KeyCode::ShiftLeft, Key::Shift, ButtonState::Released),
        ];
        assert_eq!(
            ModifierKeys::frame_start(start, delivered, ModifierKeys::default()),
            start
        );
    }

    /// Asserts that an added AltGr release clears right Alt and AltGraph from
    /// the frame start.
    ///
    /// Case: a user on a Windows AltGr layout presses Shift while AltGr is
    /// held, and Bevy releases AltRight with the AltGraph logical key.
    #[test]
    fn an_added_altgr_release_clears_alt_right_and_alt_graph() {
        let start = keys_with(&[(KeyCode::AltRight, Key::AltGraph)]);
        let delivered = [DeliveredKey::new(
            KeyCode::ShiftLeft,
            Key::Shift,
            ButtonState::Pressed,
        )];
        let end = keys_with(&[(KeyCode::ShiftLeft, Key::Shift)]);
        assert_eq!(
            ModifierKeys::frame_start(start, delivered, end),
            ModifierKeys::default()
        );
    }

    /// Asserts that a window event other than a key event decodes to no
    /// delivered key.
    ///
    /// Case: the window loses focus while the frame-start system reads the
    /// raw winit events.
    #[test]
    fn a_window_event_other_than_a_key_is_not_delivered() {
        assert_eq!(DeliveredKey::from_raw(&WindowEvent::Focused(false)), None);
    }

    /// Asserts that the frame-start resource follows the input state across
    /// frames, and that an unchanged frame leaves it untouched.
    ///
    /// Case: the user presses left Shift, then holds it through another
    /// frame.
    #[test]
    fn the_frame_start_follows_button_input_between_frames() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(HeldModifiersPlugin)
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<Key>>()
            .insert_resource(OrzmaConfigsResource(OrzmaConfigs::default()));
        app.update();
        assert_eq!(
            *app.world().resource::<FrameStartModifiers>(),
            FrameStartModifiers::default()
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ShiftLeft);
        app.update();
        assert!(
            app.world()
                .resource::<FrameStartModifiers>()
                .keys()
                .held()
                .shift
        );
        let tick = app
            .world()
            .resource_ref::<FrameStartModifiers>()
            .last_changed();
        app.update();
        assert_eq!(
            app.world()
                .resource_ref::<FrameStartModifiers>()
                .last_changed(),
            tick
        );
    }
}
