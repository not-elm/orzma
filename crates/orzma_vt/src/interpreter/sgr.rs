//! The `SGR` half of [`Pen`].
//!
//! `Pen` itself lives in `screen::cell`, which knows nothing about
//! `vtparse`. Applying a control sequence to it is interpretation, so
//! the `impl` sits here and the screen layer stays free of the parser.

use crate::device::color::Color;
use crate::interpreter::csi::CsiParams;
use crate::screen::cell::Pen;
use crate::screen::grid::run::Style;
use vtparse::CsiParam;

/// The most subparameters any form this terminal answers carries.
///
/// The longest is the direct colour with the tolerance tail ITU T.416
/// permits: `2 : Pi : r : g : b : tolerance : colour-space` is eight
/// counting the selector.
const MAX_SUBPARAMS: usize = 8;

impl Pen {
    /// The pen this one becomes after `params`, applied left to right.
    ///
    /// # Invariants
    ///
    /// The attributes this terminal cannot paint are dropped in pairs —
    /// blink (`5`, `6`, `25`), overline (`53`, `55`), and the underline
    /// colour's reset (`59`) — because implementing a set without its
    /// cancellation leaves an attribute no sequence can clear.
    ///
    /// The underline variants `4:1` through `4:5` all become the one
    /// underline this terminal draws; `4:0` cancels it.
    ///
    /// # Notes
    ///
    /// `vtparse` truncates a parameter list at `MAX_PARAMS = 32`, and
    /// separators occupy slots, so roughly sixteen values survive. The
    /// truncation is not detectable here: `csi_dispatch` receives
    /// `ignored_excess_intermediates` as its `parameters_truncated`
    /// argument, never `params_full`. A long enough sequence loses its
    /// tail silently, and a cut can land inside a direct colour.
    pub(crate) fn applied(self, params: &CsiParams<'_>) -> Self {
        let mut pen = self;
        for tokens in params.groups() {
            if let Some(group) = Group::decode(tokens) {
                pen.apply_group(&group);
            }
        }
        pen
    }

    /// Applies one group to this pen.
    fn apply_group(&mut self, group: &Group) {
        let subs = group.as_slice();
        let attribute = subs.first().copied().flatten().unwrap_or(0);
        match attribute {
            0 => *self = Self::default(),
            1 => self.style.insert(Style::BOLD),
            2 => self.style.insert(Style::DIM),
            3 => self.style.insert(Style::ITALIC),
            4 => self
                .style
                .set(Style::UNDERLINE, subs.get(1).copied().flatten() != Some(0)),
            7 => self.style.insert(Style::REVERSE),
            8 => self.style.insert(Style::HIDDEN),
            9 => self.style.insert(Style::STRIKE),
            21 => self.style.insert(Style::UNDERLINE),
            22 => self.style.remove(Style::BOLD | Style::DIM),
            23 => self.style.remove(Style::ITALIC),
            24 => self.style.remove(Style::UNDERLINE),
            27 => self.style.remove(Style::REVERSE),
            28 => self.style.remove(Style::HIDDEN),
            29 => self.style.remove(Style::STRIKE),
            30..=37 => self.fg = Color::Indexed((attribute - 30) as u8),
            39 => self.fg = Color::DefaultForeground,
            40..=47 => self.bg = Color::Indexed((attribute - 40) as u8),
            49 => self.bg = Color::DefaultBackground,
            90..=97 => self.fg = Color::Indexed((attribute - 90 + 8) as u8),
            100..=107 => self.bg = Color::Indexed((attribute - 100 + 8) as u8),
            // NOTE: These carry no operands, so dropping them cannot
            // desynchronise the walk. They are listed rather than left
            // to the fallthrough so the set/reset pairing stays
            // greppable: adding 5 without 25 would be visible here.
            5 | 6 | 25 | 53 | 55 | 59 => {}
            _ => {}
        }
    }
}

/// One `;`-separated group, decoded once.
struct Group {
    subs: [Option<u16>; MAX_SUBPARAMS],
    len: usize,
}

impl Group {
    /// The group `tokens` spell; `None` for a group carrying a token
    /// that is neither an integer nor a subparameter separator, or more
    /// subparameters than any answered form.
    ///
    /// # Invariants
    ///
    /// A group longer than [`MAX_SUBPARAMS`] is discarded whole rather
    /// than truncated. The longest form this terminal answers is eight
    /// subparameters, so a longer one is malformed rather than a
    /// tolerance tail to ignore.
    fn decode(tokens: &[CsiParam]) -> Option<Self> {
        let mut subs = [None; MAX_SUBPARAMS];
        let mut len = 0;
        for sub in tokens.split(|token| matches!(token, CsiParam::P(b':'))) {
            if len == MAX_SUBPARAMS {
                return None;
            }
            subs[len] = match sub {
                [] => None,
                [CsiParam::Integer(value)] => Some(u16::try_from(*value).unwrap_or(u16::MAX)),
                _ => return None,
            };
            len += 1;
        }
        Some(Self { subs, len })
    }

    /// The decoded subparameters, an empty one reading as `None`.
    fn as_slice(&self) -> &[Option<u16>] {
        &self.subs[..self.len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one SGR sequence's parameters against a fresh pen.
    fn applied(tokens: &[CsiParam]) -> Pen {
        Pen::default().applied(&CsiParams::parse(tokens))
    }

    /// Builds the token slice a `;`-separated parameter list spells.
    fn semicolons(values: &[i64]) -> Vec<CsiParam> {
        let mut tokens = Vec::new();
        for (index, value) in values.iter().enumerate() {
            if index > 0 {
                tokens.push(CsiParam::P(b';'));
            }
            tokens.push(CsiParam::Integer(*value));
        }
        tokens
    }

    /// Asserts that an empty parameter list resets the pen, which is
    /// what a bare `CSI m` spells.
    ///
    /// Case: a program clears every attribute before drawing its own.
    #[test]
    fn an_empty_parameter_list_resets_the_pen() {
        let pen = Pen {
            style: Style::BOLD,
            fg: Color::Indexed(1),
            ..Pen::default()
        };
        assert_eq!(pen.applied(&CsiParams::parse(&[])), Pen::default());
    }

    /// Asserts that an omitted slot reads as a reset rather than
    /// vanishing, so `1;;31` ends plain red.
    ///
    /// Case: a program builds its parameter list by joining fields and
    /// leaves one empty.
    #[test]
    fn an_omitted_slot_reads_as_a_reset() {
        let tokens = [
            CsiParam::Integer(1),
            CsiParam::P(b';'),
            CsiParam::P(b';'),
            CsiParam::Integer(31),
        ];
        let pen = applied(&tokens);
        assert!(!pen.style.contains(Style::BOLD));
        assert_eq!(pen.fg, Color::Indexed(1));
    }

    /// Asserts that parameters apply left to right, so a reset in the
    /// middle discards what came before it.
    ///
    /// Case: a program emits `CSI 1;0;31 m` and expects plain red.
    #[test]
    fn parameters_apply_left_to_right() {
        let pen = applied(&semicolons(&[1, 0, 31]));
        assert!(!pen.style.contains(Style::BOLD));
        assert_eq!(pen.fg, Color::Indexed(1));
    }

    /// Asserts that each attribute number sets its own flag.
    ///
    /// Case: a pager turns on every emphasis it knows before drawing a
    /// heading.
    #[test]
    fn each_attribute_number_sets_its_flag() {
        let pen = applied(&semicolons(&[1, 2, 3, 4, 7, 8, 9]));
        assert_eq!(
            pen.style,
            Style::BOLD
                | Style::DIM
                | Style::ITALIC
                | Style::UNDERLINE
                | Style::REVERSE
                | Style::HIDDEN
                | Style::STRIKE
        );
    }

    /// Asserts that `SGR 22` clears both bold and faint, which share
    /// one cancellation.
    ///
    /// Case: a program that set both intensities returns to normal
    /// weight with the single number the standard gives it.
    #[test]
    fn the_intensity_reset_clears_both_bold_and_dim() {
        let pen = applied(&semicolons(&[1, 2, 22]));
        assert!(!pen.style.contains(Style::BOLD));
        assert!(!pen.style.contains(Style::DIM));
    }

    /// Asserts that each cancellation number clears its own flag and
    /// leaves the others.
    ///
    /// Case: a program turns off italics without disturbing the
    /// underline it drew the same heading with.
    #[test]
    fn each_cancellation_clears_only_its_flag() {
        let pen = applied(&semicolons(&[3, 4, 23]));
        assert!(!pen.style.contains(Style::ITALIC));
        assert!(pen.style.contains(Style::UNDERLINE));
    }

    /// Asserts that the doubly-underlined number degrades to a plain
    /// underline, the only one this terminal can paint.
    ///
    /// Case: a compiler underlines a diagnostic span twice to mark a
    /// secondary label.
    #[test]
    fn the_doubly_underlined_number_degrades_to_a_plain_underline() {
        assert!(applied(&semicolons(&[21])).style.contains(Style::UNDERLINE));
    }

    /// Asserts that the basic colour numbers select palette slots 0
    /// through 7 on their own axis.
    ///
    /// Case: a build tool prints a red error on a green banner.
    #[test]
    fn the_basic_colour_numbers_select_the_first_eight_slots() {
        let pen = applied(&semicolons(&[31, 42]));
        assert_eq!(pen.fg, Color::Indexed(1));
        assert_eq!(pen.bg, Color::Indexed(2));
    }

    /// Asserts that the bright colour numbers select palette slots 8
    /// through 15 rather than lighting bold.
    ///
    /// Case: a prompt paints itself bright cyan without asking for a
    /// heavier font.
    #[test]
    fn the_bright_colour_numbers_select_the_upper_eight_slots() {
        let pen = applied(&semicolons(&[96, 101]));
        assert_eq!(pen.fg, Color::Indexed(14));
        assert_eq!(pen.bg, Color::Indexed(9));
        assert!(!pen.style.contains(Style::BOLD));
    }

    /// Asserts that the default colour numbers return each axis to the
    /// terminal's own colour.
    ///
    /// Case: a program finishes a coloured run and returns to the
    /// window's foreground.
    #[test]
    fn the_default_colour_numbers_return_each_axis() {
        let pen = applied(&semicolons(&[31, 42, 39, 49]));
        assert_eq!(pen.fg, Color::DefaultForeground);
        assert_eq!(pen.bg, Color::DefaultBackground);
    }

    /// Asserts that the attributes this terminal cannot paint leave the
    /// pen untouched, set and reset alike.
    ///
    /// Case: an application asks for blinking text and an overline,
    /// neither of which this terminal draws.
    #[test]
    fn the_unpaintable_attributes_leave_the_pen_untouched() {
        assert_eq!(
            applied(&semicolons(&[5, 6, 25, 53, 55, 59])),
            Pen::default()
        );
    }

    /// Asserts that an unknown number is ignored on its own, leaving
    /// the attributes around it in force.
    ///
    /// Case: an application emits an attribute from a later standard
    /// between two this terminal answers.
    #[test]
    fn an_unknown_number_is_ignored_on_its_own() {
        let pen = applied(&semicolons(&[1, 99, 31]));
        assert!(pen.style.contains(Style::BOLD));
        assert_eq!(pen.fg, Color::Indexed(1));
    }

    /// Asserts that a colon group on an attribute that takes no
    /// subparameters uses the first one and ignores the rest.
    ///
    /// Case: a program spells a curly underline as `CSI 4:3 m`, whose
    /// variant this terminal cannot paint.
    #[test]
    fn a_colon_group_on_a_plain_attribute_uses_its_first_subparameter() {
        let tokens = [
            CsiParam::Integer(4),
            CsiParam::P(b':'),
            CsiParam::Integer(3),
        ];
        assert!(applied(&tokens).style.contains(Style::UNDERLINE));
    }

    /// Asserts that the zero underline variant cancels the underline
    /// rather than drawing one.
    ///
    /// Case: an editor clears a squiggle with the colon spelling it
    /// drew the squiggle with.
    #[test]
    fn the_zero_underline_variant_cancels_the_underline() {
        let tokens = [
            CsiParam::Integer(4),
            CsiParam::P(b':'),
            CsiParam::Integer(0),
        ];
        let pen = Pen {
            style: Style::UNDERLINE,
            ..Pen::default()
        };
        let after = pen.applied(&CsiParams::parse(&tokens));
        assert!(!after.style.contains(Style::UNDERLINE));
    }

    /// Asserts that a group carrying a token that is neither an integer
    /// nor a subparameter separator is discarded.
    ///
    /// Case: a malformed stream lands a comparison byte inside the
    /// parameter list.
    #[test]
    fn a_group_with_a_stray_token_is_discarded() {
        let tokens = [
            CsiParam::Integer(1),
            CsiParam::P(b';'),
            CsiParam::P(b'<'),
            CsiParam::P(b';'),
            CsiParam::Integer(31),
        ];
        let pen = applied(&tokens);
        assert!(pen.style.contains(Style::BOLD));
        assert_eq!(pen.fg, Color::Indexed(1));
    }
}
