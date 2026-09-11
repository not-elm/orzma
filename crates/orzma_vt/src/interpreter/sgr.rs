//! The `SGR` half of [`Pen`].

use crate::device::color::Color;
use crate::interpreter::csi::CsiParams;
use crate::screen::cell::Pen;
use crate::screen::grid::run::Style;
use std::ops::ControlFlow;
use vtparse::CsiParam;

/// The most subparameters any form this terminal answers carries.
///
/// The longest is the direct colour with the tolerance tail ITU T.416
/// permits: `2 : Pi : r : g : b : unused : tolerance : colour-space` is
/// nine counting the selector.
const MAX_SUBPARAMS: usize = 9;

impl Pen {
    /// The pen this one becomes after `params`, applied left to right.
    ///
    /// The attributes this terminal cannot paint are dropped in pairs:
    /// blink (`5`, `6`, `25`), overline (`53`, `55`), and the underline
    /// colour's reset (`59`).
    ///
    /// The underline variants `4:1` through `4:5` all become the one
    /// underline this terminal draws; `4:0` cancels it.
    ///
    /// `vtparse` keeps only the first 32 slots of a parameter list,
    /// separators included, so roughly sixteen values survive. A long
    /// enough sequence loses its tail silently, and a cut can land
    /// inside a direct colour.
    pub(crate) fn applied(self, params: &CsiParams<'_>) -> Self {
        let mut pen = self;
        let mut groups = params.groups();
        while let Some(tokens) = groups.next() {
            let Some(group) = Group::decode(tokens) else {
                continue;
            };
            if pen.apply_group(&mut groups, &group).is_break() {
                break;
            }
        }
        pen
    }

    /// Applies one group; `Break` when the rest of the sequence can no
    /// longer be resynchronised and must be abandoned.
    fn apply_group<'a>(
        &mut self,
        groups: &mut impl Iterator<Item = &'a [CsiParam]>,
        group: &Group,
    ) -> ControlFlow<()> {
        let subs = group.as_slice();
        let attribute = subs.first().copied().flatten().unwrap_or(0);
        match attribute {
            0 => *self = Self::default(),
            1 => self.style.insert(Style::BOLD),
            2 => self.style.insert(Style::DIM),
            3 => self.style.insert(Style::ITALIC),
            4 => self.style.set(
                Style::UNDERLINE,
                subs.get(1).map(|sub| sub.unwrap_or(0)) != Some(0),
            ),
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
            38 => match Self::read_color(groups, group) {
                ColorRead::Done(color) => self.fg = color,
                ColorRead::Discarded => {}
                ColorRead::Abandon => return ControlFlow::Break(()),
            },
            48 => match Self::read_color(groups, group) {
                ColorRead::Done(color) => self.bg = color,
                ColorRead::Discarded => {}
                ColorRead::Abandon => return ControlFlow::Break(()),
            },
            // NOTE: The underline colour has no home in `Pen`, but its
            // operands must still be consumed. Ignoring the selector
            // alone would let `58;2;255;0;0` read as faint, an unknown
            // number, and two full resets.
            58 => match Self::read_color(groups, group) {
                ColorRead::Done(_) | ColorRead::Discarded => {}
                ColorRead::Abandon => return ControlFlow::Break(()),
            },
            // NOTE: These carry no operands, so dropping them cannot
            // desynchronise the walk. They are listed rather than left
            // to the fallthrough so the set/reset pairing stays
            // greppable: adding 5 without 25 would be visible here.
            5 | 6 | 25 | 53 | 55 | 59 => {}
            _ => {}
        }
        ControlFlow::Continue(())
    }

    /// Reads the colour a selector group introduces, consuming the
    /// following groups when the legacy spelling needs them.
    fn read_color<'a>(
        groups: &mut impl Iterator<Item = &'a [CsiParam]>,
        group: &Group,
    ) -> ColorRead {
        let subs = group.as_slice();
        if subs.len() > 1 {
            return Color::from_sgr_group(&subs[1..]).map_or(ColorRead::Discarded, ColorRead::Done);
        }
        let Some(selector) = Self::next_value(groups) else {
            return ColorRead::Discarded;
        };
        let operand_count = match selector {
            5 => 1,
            2 => 3,
            _ => return ColorRead::Abandon,
        };
        let mut operands = [0u16; 3];
        for slot in operands.iter_mut().take(operand_count) {
            let Some(value) = Self::next_value(groups) else {
                return ColorRead::Discarded;
            };
            *slot = value;
        }
        Color::from_sgr(selector, &operands[..operand_count])
            .map_or(ColorRead::Discarded, ColorRead::Done)
    }

    /// The next group's first value, an omitted or malformed one
    /// reading as zero; `None` only when the list has ended.
    fn next_value<'a>(groups: &mut impl Iterator<Item = &'a [CsiParam]>) -> Option<u16> {
        let tokens = groups.next()?;
        let value = Group::decode(tokens)
            .and_then(|group| group.as_slice().first().copied().flatten())
            .unwrap_or(0);
        Some(value)
    }
}

/// What reading a colour selector produced.
enum ColorRead {
    /// A colour to apply.
    Done(Color),
    /// The operands were consumed and the colour rejected.
    Discarded,
    /// The selector is not one this terminal answers, so the rest of the
    /// sequence cannot be resynchronised.
    Abandon,
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
    /// A group longer than [`MAX_SUBPARAMS`] is discarded whole rather
    /// than truncated.
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
    use crate::device::color::Rgb;

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

    /// Builds the token slice a colon-separated group spells, an entry
    /// of `None` standing for an omitted subparameter.
    fn colons(subs: &[Option<i64>]) -> Vec<CsiParam> {
        let mut tokens = Vec::new();
        for (index, sub) in subs.iter().enumerate() {
            if index > 0 {
                tokens.push(CsiParam::P(b':'));
            }
            if let Some(value) = sub {
                tokens.push(CsiParam::Integer(*value));
            }
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

    /// Asserts that an omitted underline variant reads as zero and
    /// cancels the underline, the same as spelling the zero out.
    ///
    /// Case: an editor builds `CSI 4:Pv m` from a variant field that
    /// came out empty.
    #[test]
    fn an_omitted_underline_variant_cancels_the_underline() {
        let tokens = [CsiParam::Integer(4), CsiParam::P(b':')];
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

    /// Asserts that both spellings of an indexed colour reach the same
    /// palette slot.
    ///
    /// Case: one application uses the legacy semicolon spelling and
    /// another the standard colon spelling for the same colour.
    #[test]
    fn both_spellings_of_an_indexed_colour_agree() {
        assert_eq!(applied(&semicolons(&[38, 5, 196])).fg, Color::Indexed(196));
        assert_eq!(
            applied(&colons(&[Some(38), Some(5), Some(196)])).fg,
            Color::Indexed(196)
        );
    }

    /// Asserts that every spelling of a direct colour reaches the same
    /// value, including the omitted colour-space slot and the shortened
    /// form many programs emit.
    ///
    /// Case: terminfo's `alacritty-direct` emits the omitted-slot form
    /// while other programs drop the slot entirely.
    #[test]
    fn every_spelling_of_a_direct_colour_agrees() {
        let expected = Color::Rgb(Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(applied(&semicolons(&[38, 2, 1, 2, 3])).fg, expected);
        assert_eq!(
            applied(&colons(&[
                Some(38),
                Some(2),
                None,
                Some(1),
                Some(2),
                Some(3)
            ]))
            .fg,
            expected
        );
        assert_eq!(
            applied(&colons(&[Some(38), Some(2), Some(1), Some(2), Some(3)])).fg,
            expected
        );
    }

    /// Asserts that subparameters after the blue channel are ignored.
    ///
    /// Case: an application spells its colour with the full T.416 form,
    /// whose three fields after blue are the unused slot, the tolerance,
    /// and the colour space the tolerance is measured in.
    #[test]
    fn subparameters_after_blue_are_ignored() {
        let expected = Color::Rgb(Rgb { r: 1, g: 2, b: 3 });
        let two_field_tail = colons(&[
            Some(38),
            Some(2),
            None,
            Some(1),
            Some(2),
            Some(3),
            Some(0),
            Some(0),
        ]);
        let three_field_tail = colons(&[
            Some(38),
            Some(2),
            None,
            Some(1),
            Some(2),
            Some(3),
            Some(0),
            Some(0),
            Some(0),
        ]);
        assert_eq!(applied(&two_field_tail).fg, expected);
        assert_eq!(applied(&three_field_tail).fg, expected);
    }

    /// Asserts that the background selector reaches the background
    /// axis and leaves the foreground alone.
    ///
    /// Case: a syntax highlighter paints a selection background.
    #[test]
    fn the_background_selector_reaches_the_background() {
        let pen = applied(&semicolons(&[48, 5, 17]));
        assert_eq!(pen.bg, Color::Indexed(17));
        assert_eq!(pen.fg, Color::DefaultForeground);
    }

    /// Asserts that a colour component beyond a byte is rejected rather
    /// than saturated, leaving the axis as it was.
    ///
    /// Case: a program computes a component from an unbounded value and
    /// emits `CSI 38;5;99999 m`.
    #[test]
    fn an_oversized_colour_component_is_rejected() {
        assert_eq!(
            applied(&semicolons(&[38, 5, 99999])).fg,
            Color::DefaultForeground
        );
        assert_eq!(
            applied(&semicolons(&[38, 2, 1, 300, 3])).fg,
            Color::DefaultForeground
        );
    }

    /// Asserts that a malformed direct colour leaves the pen as it was
    /// rather than applying part of itself.
    ///
    /// Case: a truncated sequence loses its blue channel.
    #[test]
    fn a_malformed_direct_colour_leaves_the_pen_alone() {
        assert_eq!(
            applied(&semicolons(&[31, 38, 2, 1, 2])).fg,
            Color::Indexed(1)
        );
    }

    /// Asserts that the walk continues past a malformed colour in the
    /// colon spelling.
    ///
    /// Case: an application emits a truncated colon colour followed by
    /// an attribute it still expects to take effect.
    #[test]
    fn a_malformed_colon_colour_does_not_stop_the_walk() {
        let mut tokens = colons(&[Some(38), Some(2), None, Some(1)]);
        tokens.push(CsiParam::P(b';'));
        tokens.push(CsiParam::Integer(1));
        assert!(applied(&tokens).style.contains(Style::BOLD));
    }

    /// Asserts that an unknown colour selector in the semicolon spelling
    /// abandons the rest of the sequence.
    ///
    /// Case: an application asks for a colour space this terminal does
    /// not answer, then spells an attribute after it.
    #[test]
    fn an_unknown_colour_selector_abandons_the_rest() {
        let pen = applied(&semicolons(&[38, 9, 1]));
        assert!(!pen.style.contains(Style::BOLD));
        assert_eq!(pen.fg, Color::DefaultForeground);
    }

    /// Asserts that an omitted operand of a direct colour reads as zero
    /// rather than ending the colour, so the operands after it never
    /// reach the pen as ordinary attributes.
    ///
    /// Case: a script builds `CSI 38;2;R;G;B m` by joining shell
    /// variables and leaves the red one unset.
    #[test]
    fn an_omitted_colour_operand_reads_as_zero() {
        let mut tokens = semicolons(&[38, 2]);
        tokens.push(CsiParam::P(b';'));
        tokens.push(CsiParam::P(b';'));
        tokens.push(CsiParam::Integer(0));
        tokens.push(CsiParam::P(b';'));
        tokens.push(CsiParam::Integer(0));
        let pen = Pen {
            style: Style::BOLD,
            ..Pen::default()
        };
        let after = pen.applied(&CsiParams::parse(&tokens));
        assert_eq!(after.fg, Color::Rgb(Rgb { r: 0, g: 0, b: 0 }));
        assert!(after.style.contains(Style::BOLD));
    }

    /// Asserts that an omitted index of an indexed colour reads as zero
    /// and leaves the group after it to apply on its own.
    ///
    /// Case: an application emits `CSI 38;5;;3 m`, whose index field
    /// came out empty.
    #[test]
    fn an_omitted_colour_index_reads_as_zero() {
        let mut tokens = semicolons(&[38, 5]);
        tokens.push(CsiParam::P(b';'));
        tokens.push(CsiParam::P(b';'));
        tokens.push(CsiParam::Integer(3));
        let pen = applied(&tokens);
        assert_eq!(pen.fg, Color::Indexed(0));
        assert!(pen.style.contains(Style::ITALIC));
    }

    /// Asserts that a colour selector at the end of the list is
    /// discarded without disturbing what came before it.
    ///
    /// Case: `MAX_PARAMS` truncation cuts a sequence immediately after
    /// its colour selector.
    #[test]
    fn a_colour_selector_at_the_end_is_discarded() {
        let pen = applied(&semicolons(&[1, 38]));
        assert!(pen.style.contains(Style::BOLD));
        assert_eq!(pen.fg, Color::DefaultForeground);
    }

    /// Asserts that the underline colour consumes its operands even
    /// though this terminal cannot paint it, so the operands never read
    /// as ordinary attributes.
    ///
    /// Case: Vim's default `t_8u` emits `CSI 58;2;r;g;b m` for a
    /// coloured undercurl, and the italic that follows it on the same
    /// line is the next attribute the editor asks for.
    #[test]
    fn the_underline_colour_consumes_its_operands() {
        let pen = Pen {
            style: Style::BOLD,
            fg: Color::Indexed(1),
            ..Pen::default()
        };
        let after = pen.applied(&CsiParams::parse(&semicolons(&[58, 2, 255, 0, 0, 3])));
        assert_eq!(after.fg, pen.fg);
        assert!(after.style.contains(Style::BOLD));
        assert!(after.style.contains(Style::ITALIC));
    }

    /// Asserts that the underline colour's colon spelling is consumed
    /// whole as well.
    ///
    /// Case: an editor emits the standard colon spelling for the same
    /// coloured underline.
    #[test]
    fn the_underline_colour_consumes_its_colon_spelling() {
        let tokens = colons(&[Some(58), Some(2), None, Some(255), Some(0), Some(0)]);
        assert_eq!(applied(&tokens), Pen::default());
    }
}
