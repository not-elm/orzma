//! Graphic character set designation (SCS) and invocation (locking and
//! single shifts) for one screen. Only the GL half of the code table is
//! modelled.

use std::ops::{Index, IndexMut};

/// A code position already mapped through the character set invoked into GL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GraphicChar(pub char);

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

impl CharacterSet {
    /// The character set an `SCS` final character selects.
    ///
    /// A final this terminal has no set for resolves to ASCII instead
    /// of leaving the previous designation standing.
    pub fn from_dscs(dscs: u8) -> Self {
        match dscs {
            b'B' => Self::Ascii,
            b'0' => Self::DecSpecialGraphics,
            _ => Self::Ascii,
        }
    }

    /// The graphic character this set shows at `c`.
    fn graphic(self, c: char) -> GraphicChar {
        let graphic = match self {
            Self::Ascii => c,
            Self::DecSpecialGraphics => match c {
                '_' => ' ',
                '`' => '◆',
                'a' => '▒',
                'b' => '␉',
                'c' => '␌',
                'd' => '␍',
                'e' => '␊',
                'f' => '°',
                'g' => '±',
                'h' => '␤',
                'i' => '␋',
                'j' => '┘',
                'k' => '┐',
                'l' => '┌',
                'm' => '└',
                'n' => '┼',
                'o' => '⎺',
                'p' => '⎻',
                'q' => '─',
                'r' => '⎼',
                's' => '⎽',
                't' => '├',
                'u' => '┤',
                'v' => '┴',
                'w' => '┬',
                'x' => '│',
                'y' => '≤',
                'z' => '≥',
                '{' => 'π',
                '|' => '≠',
                '}' => '£',
                '~' => '·',
                _ => c,
            },
        };
        GraphicChar(graphic)
    }
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

impl GCode {
    /// The G code an `SCS` designator selects; `None` for a byte that
    /// designates nothing this terminal implements.
    ///
    /// The 96-character designators `-`, `.`, and `/` are among the
    /// bytes answered with `None`.
    pub fn from_designator(designator: u8) -> Option<Self> {
        match designator {
            b'(' => Some(Self::G0),
            b')' => Some(Self::G1),
            b'*' => Some(Self::G2),
            b'+' => Some(Self::G3),
            _ => None,
        }
    }
}

/// The G code a single shift invokes into GL for one graphic character.
///
/// SS2 and SS3 are the only single shifts the VT220 defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleShift {
    /// `SS2` (`0x8E`, `ESC N`) invokes G2.
    G2,
    /// `SS3` (`0x8F`, `ESC O`) invokes G3.
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

/// The character set mapping one screen prints characters through.
///
/// # Invariants
///
/// A `pending_single_shift` outranks `gl` for exactly one graphic
/// character: [`Self::translate`] clears it as it maps that character,
/// and a locking shift leaves it armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CharacterSetMapping {
    /// The G code the latest locking shift invoked into GL.
    pub gl: GCode,
    /// The G code a pending `SS2` or `SS3` invokes into GL for the next
    /// graphic character.
    pub pending_single_shift: Option<SingleShift>,
    /// The character set `SCS` designated to each G code.
    pub g_sets: GSets,
}

impl CharacterSetMapping {
    /// Designates `character_set` to `g_code`.
    ///
    /// # Control Functions
    ///
    /// - `SCS` (`ESC ( Dscs`, `ESC ) Dscs`, `ESC * Dscs`, `ESC + Dscs`)
    pub fn designate(&mut self, g_code: GCode, character_set: CharacterSet) {
        self.g_sets[g_code] = character_set;
    }

    /// Invokes `g_code` into GL.
    ///
    /// TODO: implement `LS1R`, `LS2R`, and `LS3R` once an 8-bit input
    /// mode exists.
    ///
    /// # Control Functions
    ///
    /// - `LS0` (`SI`, `0x0F`)
    /// - `LS1` (`SO`, `0x0E`)
    /// - `LS2` (`ESC n`)
    /// - `LS3` (`ESC o`)
    pub fn invoke(&mut self, g_code: GCode) {
        self.gl = g_code;
    }

    /// Invokes `single_shift` into GL for the next graphic character.
    ///
    /// # Control Functions
    ///
    /// - `SS2` (`0x8E`, `ESC N`)
    /// - `SS3` (`0x8F`, `ESC O`)
    pub fn single_shift(&mut self, single_shift: SingleShift) {
        self.pending_single_shift = Some(single_shift);
    }

    /// The graphic character `c` prints as, consuming a pending single
    /// shift.
    pub fn translate(&mut self, c: char) -> GraphicChar {
        let g_code = match self.pending_single_shift.take() {
            Some(SingleShift::G2) => GCode::G2,
            Some(SingleShift::G3) => GCode::G3,
            None => self.gl,
        };
        self.g_sets[g_code].graphic(c)
    }

    /// Restores the power-up designations and invocations, dropping any
    /// pending single shift.
    ///
    /// TODO: reach this reset from DECSTR (CSI ! p).
    #[expect(
        dead_code,
        reason = "the executor reaches this reset once DECSTR lands; RIS goes through `Screen::reset`"
    )]
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod from_designator {
        use super::*;

        /// Asserts that each of the four `SCS` designators selects its
        /// own G code.
        ///
        /// Case: an application designates line drawing into a
        /// different bank with each of `ESC ( 0`, `ESC ) 0`, `ESC * 0`,
        /// and `ESC + 0`.
        #[test]
        fn each_designator_selects_its_own_g_code() {
            assert_eq!(GCode::from_designator(b'('), Some(GCode::G0));
            assert_eq!(GCode::from_designator(b')'), Some(GCode::G1));
            assert_eq!(GCode::from_designator(b'*'), Some(GCode::G2));
            assert_eq!(GCode::from_designator(b'+'), Some(GCode::G3));
        }

        /// Asserts that the 96-character designators select no G code
        /// rather than folding onto G1 through G3 alongside their
        /// 94-character spellings.
        ///
        /// Case: an application designates ISO Latin-1 supplemental
        /// into G1 with `ESC - A`.
        #[test]
        fn a_ninety_six_character_designator_selects_no_g_code() {
            assert_eq!(GCode::from_designator(b'-'), None);
            assert_eq!(GCode::from_designator(b'.'), None);
            assert_eq!(GCode::from_designator(b'/'), None);
        }
    }

    mod from_dscs {
        use super::*;

        /// Asserts that the two finals this terminal has sets for
        /// select those sets.
        ///
        /// Case: an application draws a box with `ESC ( 0` and then
        /// restores letters with `ESC ( B`.
        #[test]
        fn a_supported_final_selects_its_set() {
            assert_eq!(CharacterSet::from_dscs(b'B'), CharacterSet::Ascii);
            assert_eq!(
                CharacterSet::from_dscs(b'0'),
                CharacterSet::DecSpecialGraphics
            );
        }

        /// Asserts that a final with no set behind it resolves to ASCII
        /// rather than leaving the previous designation standing.
        ///
        /// Case: an application running under a Finnish locale
        /// designates its national replacement set with `ESC ( C`.
        #[test]
        fn an_unsupported_final_falls_back_to_ascii() {
            assert_eq!(CharacterSet::from_dscs(b'C'), CharacterSet::Ascii);
            assert_eq!(CharacterSet::from_dscs(b'A'), CharacterSet::Ascii);
        }
    }

    mod translate {
        use super::*;

        /// Asserts that a code position maps through the set the
        /// locking shift invoked into GL.
        ///
        /// Case: an application designates line drawing into G1, shifts
        /// to it, and draws a horizontal rule out of `q` characters.
        #[test]
        fn a_code_position_maps_through_the_set_invoked_into_gl() {
            let mut mapping = CharacterSetMapping::default();
            mapping.designate(GCode::G1, CharacterSet::DecSpecialGraphics);
            assert_eq!(mapping.translate('q'), GraphicChar('q'));

            mapping.invoke(GCode::G1);

            assert_eq!(mapping.translate('q'), GraphicChar('─'));
        }

        /// Asserts that a single shift maps one character and then
        /// stops applying.
        ///
        /// Case: an application prints a single degree sign out of G2
        /// with `SS2 f` in the middle of a line of ordinary text.
        #[test]
        fn a_single_shift_is_spent_on_one_character() {
            let mut mapping = CharacterSetMapping::default();
            mapping.designate(GCode::G2, CharacterSet::DecSpecialGraphics);
            mapping.single_shift(SingleShift::G2);

            assert_eq!(mapping.translate('f'), GraphicChar('°'));

            assert_eq!(mapping.pending_single_shift, None);
            assert_eq!(mapping.translate('f'), GraphicChar('f'));
        }
    }

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
                let mut state = CharacterSetMapping::default();
                state.designate(g_code, DecSpecialGraphics);
                assert_eq!(state.g_sets, expected);
                assert_eq!(state.g_sets[g_code], DecSpecialGraphics);
            }
        }

        /// Asserts that a second designation to the same G code replaces
        /// the character set the first one put there.
        ///
        /// Case: an application finishes drawing a box with DEC Special
        /// Graphics on G0 and emits `ESC ( B`.
        #[test]
        fn redesignating_a_g_code_replaces_the_previous_set() {
            let mut state = CharacterSetMapping::default();
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
            let mut state = CharacterSetMapping {
                gl: GCode::G1,
                pending_single_shift: Some(SingleShift::G2),
                ..Default::default()
            };

            state.designate(GCode::G2, DecSpecialGraphics);

            assert_eq!(
                (state.gl, state.pending_single_shift),
                (GCode::G1, Some(SingleShift::G2))
            );
        }
    }

    mod invoke {
        use super::CharacterSet::{Ascii, DecSpecialGraphics};
        use super::*;

        /// Asserts that a locking shift replaces the G code in GL and
        /// leaves a pending single shift armed.
        ///
        /// Case: an application emits `SS2`, then `SO` before the
        /// character the single shift applies to.
        #[test]
        fn invoke_replaces_gl_and_leaves_a_pending_single_shift_armed() {
            let mut state = CharacterSetMapping::default();
            state.designate(GCode::G1, DecSpecialGraphics);
            state.pending_single_shift = Some(SingleShift::G2);

            state.invoke(GCode::G1);

            assert_eq!(
                state,
                CharacterSetMapping {
                    gl: GCode::G1,
                    pending_single_shift: Some(SingleShift::G2),
                    g_sets: GSets([Ascii, DecSpecialGraphics, Ascii, Ascii]),
                }
            );
        }
    }

    mod single_shift {
        use super::*;

        /// Asserts that a later single shift replaces the pending one
        /// rather than queueing behind it, and leaves the locking shift
        /// alone.
        ///
        /// Case: an application has shifted GL to G1 with `SO`, emits
        /// `SS2`, then changes its mind and emits `SS3` before printing.
        #[test]
        fn a_later_single_shift_replaces_the_pending_one_and_leaves_gl_alone() {
            let mut state = CharacterSetMapping {
                gl: GCode::G1,
                ..Default::default()
            };

            state.single_shift(SingleShift::G2);
            assert_eq!(state.pending_single_shift, Some(SingleShift::G2));

            state.single_shift(SingleShift::G3);

            assert_eq!(
                state,
                CharacterSetMapping {
                    gl: GCode::G1,
                    pending_single_shift: Some(SingleShift::G3),
                    g_sets: GSets::default(),
                }
            );
        }
    }
}
