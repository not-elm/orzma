//! Graphic character set designation (SCS) and invocation (locking and
//! single shifts) for one screen.
//!
//! Only the GL half of the code table is modelled. Reaching a set
//! invoked into GR takes raw `0xA0`–`0xFF` input bytes, and the UTF-8
//! parser this crate feeds on consumes that range as multi-byte
//! encoding instead, so such a set could never be selected. `LS1R`,
//! `LS2R`, and `LS3R` stay out of scope until an 8-bit input mode
//! exists.

use std::ops::{Index, IndexMut};

/// A graphic character set an application designates to a G code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CharacterSet {
    /// ASCII graphics, the designation every G code resets to.
    #[default]
    Ascii,
    /// DEC Special Graphics.
    ///
    /// This character set has about two-thirds of the ASCII graphic
    /// characters. It also has special symbols and short line segments.
    DecSpecialGraphics,
    // TODO: Support the remaining VT220 and VT510 graphic character sets.
}

/// One of the four G codes a character set is designated to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GCode {
    #[default]
    G0,
    G1,
    G2,
    G3,
}

/// The G code a single shift invokes into GL for one graphic character.
///
/// SS2 and SS3 are the only single shifts the VT220 defines, so G0 and
/// G1 are excluded by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleShift {
    /// `SS2` (`ESC N`, or `0x8E` in its 8-bit form) invokes G2.
    G2,
    /// `SS3` (`ESC O`, or `0x8F` in its 8-bit form) invokes G3.
    G3,
}

/// The character set designated to each of the four G codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GSets([CharacterSet; 4]);

impl Index<GCode> for GSets {
    type Output = CharacterSet;

    fn index(&self, g_code: GCode) -> &Self::Output {
        &self.0[g_code as usize]
    }
}

impl IndexMut<GCode> for GSets {
    fn index_mut(&mut self, g_code: GCode) -> &mut Self::Output {
        &mut self.0[g_code as usize]
    }
}

/// The character set state one screen maps printed characters through.
///
/// # Invariants
///
/// A pending `single_shift` outranks `gl` for exactly one graphic
/// character. The consumer clears it once that character is mapped, and
/// a locking shift leaves it alone: the two invocations are independent
/// state, not one field the newer control function overwrites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CharacterSetsState {
    /// The G code the latest locking shift invoked into GL.
    pub gl: GCode,
    /// The G code a pending `SS2` or `SS3` invokes into GL for the next
    /// graphic character.
    pub single_shift: Option<SingleShift>,
    /// The character set `SCS` designated to each G code.
    pub g_sets: GSets,
}

impl CharacterSetsState {
    /// Specifies the graphic character set to be used for the designated [GCode].
    pub fn designate(&mut self, g_code: GCode, character_set: CharacterSet) {
        self.g_sets[g_code] = character_set;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod designate {
        use super::CharacterSet::{Ascii, DecSpecialGraphics};
        use super::*;

        /// Asserts that a designation writes the selected G code's slot
        /// and leaves the other three holding ASCII.
        ///
        /// Case: an application prepares its banks one at a time, with
        /// `ESC ( 0`, `ESC ) 0`, `ESC * 0`, and `ESC + 0` each targeting
        /// a different G code, so that a later shift can pick the bank
        /// it wants without sending a second SCS.
        #[test]
        fn designate_updates_only_the_selected_g_code() {
            for (g_code, expected) in [
                (GCode::G0, GSets([DecSpecialGraphics, Ascii, Ascii, Ascii])),
                (GCode::G1, GSets([Ascii, DecSpecialGraphics, Ascii, Ascii])),
                (GCode::G2, GSets([Ascii, Ascii, DecSpecialGraphics, Ascii])),
                (GCode::G3, GSets([Ascii, Ascii, Ascii, DecSpecialGraphics])),
            ] {
                let mut state = CharacterSetsState::default();
                state.designate(g_code, DecSpecialGraphics);
                assert_eq!(state.g_sets, expected);
                assert_eq!(state.g_sets[g_code], DecSpecialGraphics);
            }
        }

        /// Asserts that a second designation to the same G code replaces
        /// the character set the first one put there.
        ///
        /// Case: an application finishes drawing a box with DEC Special
        /// Graphics on G0 and emits `ESC ( B`, so that the next `q`
        /// prints as a letter again instead of a horizontal line.
        #[test]
        fn redesignating_a_g_code_replaces_the_previous_set() {
            let mut state = CharacterSetsState::default();
            state.designate(GCode::G0, DecSpecialGraphics);
            assert_eq!(state.g_sets[GCode::G0], DecSpecialGraphics);
            state.designate(GCode::G0, Ascii);
            assert_eq!(state.g_sets[GCode::G0], Ascii);
        }

        /// Asserts that a designation changes neither the G code a
        /// locking shift invoked into GL nor a pending single shift.
        ///
        /// Case: an application has shifted GL to G1 for line drawing
        /// and sent `SS2` for the character it is about to print, then
        /// emits `ESC * 0` to designate G2 before that character
        /// arrives.
        #[test]
        fn designate_leaves_the_invocation_state_unchanged() {
            let mut state = CharacterSetsState::default();
            state.gl = GCode::G1;
            state.single_shift = Some(SingleShift::G2);

            state.designate(GCode::G2, DecSpecialGraphics);

            assert_eq!(
                (state.gl, state.single_shift),
                (GCode::G1, Some(SingleShift::G2))
            );
        }
    }
}
