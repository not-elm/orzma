//! The parameter view a CSI sequence is read through.

use vtparse::CsiParam;

/// One CSI sequence's parameters, split into the three things vtparse
/// packs into a single slice.
pub(crate) struct CsiParams<'a> {
    private: Option<u8>,
    values: &'a [CsiParam],
    intermediates: [u8; MAX_INTERMEDIATES],
    intermediate_len: usize,
}

impl<'a> CsiParams<'a> {
    /// Splits one `csi_dispatch` slice into its marker, values, and
    /// intermediates.
    pub fn parse(params: &'a [CsiParam]) -> Self {
        let (private, rest) = match params.first() {
            Some(CsiParam::P(byte)) if (0x3C..=0x3F).contains(byte) => (Some(*byte), &params[1..]),
            _ => (None, params),
        };
        let split = rest
            .iter()
            .position(|param| matches!(param, CsiParam::P(byte) if (0x20..=0x2F).contains(byte)))
            .unwrap_or(rest.len());
        let tail = rest[split..].iter().filter_map(|param| match param {
            CsiParam::P(byte) => Some(*byte),
            CsiParam::Integer(_) => None,
        });
        let mut intermediates = [0; MAX_INTERMEDIATES];
        let mut intermediate_len = 0;
        for (slot, byte) in intermediates.iter_mut().zip(tail) {
            *slot = byte;
            intermediate_len += 1;
        }
        Self {
            private,
            values: &rest[..split],
            intermediates,
            intermediate_len,
        }
    }

    /// The private marker a sequence opened with, if it had one.
    pub fn private(&self) -> Option<u8> {
        self.private
    }

    /// The intermediate bytes trailing the values, in the order they
    /// were sent; empty when the sequence carried none.
    pub fn intermediates(&self) -> &[u8] {
        &self.intermediates[..self.intermediate_len]
    }

    /// The `;`-separated groups, each carrying its own `:`
    /// subparameters and their separators.
    ///
    /// An empty sequence yields ONE empty group, not none.
    /// [`Self::values`] reports no slots for the same input.
    pub fn groups(&self) -> impl Iterator<Item = &'a [CsiParam]> {
        self.values
            .split(|param| matches!(param, CsiParam::P(b';')))
    }

    /// The first value of the `index`-th separated slot; `None` when the
    /// slot was omitted or does not exist.
    ///
    /// A zero reads as `Some(0)`; the caller decides whether it means
    /// the default.
    pub fn value(&self, index: usize) -> Option<u16> {
        self.values().nth(index).flatten()
    }

    /// Every separated slot in order.
    pub fn values(&self) -> impl Iterator<Item = Option<u16>> + '_ {
        let listed = (!self.values.is_empty()).then(|| self.groups());
        listed.into_iter().flatten().map(Self::first_value)
    }

    /// The saturating `u16` a slot's first integer reads as; `None` for
    /// a slot that carries none.
    fn first_value(group: &[CsiParam]) -> Option<u16> {
        group.iter().find_map(|param| match param {
            CsiParam::Integer(value) => Some(u16::try_from(*value).unwrap_or(u16::MAX)),
            _ => None,
        })
    }
}

/// How many intermediate bytes [`CsiParams`] holds, matching the cap
/// the parser collects to.
const MAX_INTERMEDIATES: usize = 2;

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a leading question mark is taken as the private
    /// marker and does not become a value.
    ///
    /// Case: an application turns on origin mode with `CSI ? 6 h`.
    #[test]
    fn a_leading_question_mark_is_the_private_marker() {
        let params = [CsiParam::P(b'?'), CsiParam::Integer(6)];
        let params = CsiParams::parse(&params);
        assert_eq!(params.private(), Some(b'?'));
        assert_eq!(params.value(0), Some(6));
    }

    /// Asserts that a leading separator is read as an omitted first
    /// value rather than as a private marker.
    ///
    /// Case: an application sets only a bottom margin with `CSI ; 3 r`.
    #[test]
    fn a_leading_separator_is_not_a_private_marker() {
        let params = [CsiParam::P(b';'), CsiParam::Integer(3)];
        let params = CsiParams::parse(&params);
        assert_eq!(params.private(), None);
        assert_eq!(params.value(0), None);
        assert_eq!(params.value(1), Some(3));
    }

    /// Asserts that a zero survives as a value instead of reading as an
    /// omitted parameter.
    ///
    /// Case: an application resets its attributes with `CSI 0 m`.
    #[test]
    fn a_zero_is_a_value_and_not_an_omission() {
        let params = [CsiParam::Integer(0)];
        let params = CsiParams::parse(&params);
        assert_eq!(params.value(0), Some(0));
    }

    /// Asserts that a value too large for the parameter type saturates
    /// rather than reading as omitted.
    ///
    /// Case: a fuzzer or a buggy program sends a line number far past
    /// any screen size.
    #[test]
    fn an_oversized_value_saturates() {
        let params = [CsiParam::Integer(999_999)];
        let params = CsiParams::parse(&params);
        assert_eq!(params.value(0), Some(u16::MAX));
    }

    /// Asserts that a trailing intermediate is reported as such instead
    /// of being counted among the values.
    ///
    /// Case: an application sends the change-attributes-in-rectangle
    /// sequence `CSI 1 ; 2 $ r`.
    #[test]
    fn a_trailing_intermediate_is_not_a_value() {
        let params = [
            CsiParam::Integer(1),
            CsiParam::P(b';'),
            CsiParam::Integer(2),
            CsiParam::P(b'$'),
        ];
        let params = CsiParams::parse(&params);
        assert_eq!(params.intermediates(), b"$");
        assert_eq!(params.value(0), Some(1));
        assert_eq!(params.value(1), Some(2));
    }

    /// Asserts that the last byte of the intermediate range is read as
    /// an intermediate rather than swallowed by the values.
    ///
    /// Case: an application sends `CSI 1 ; 2 / r`, a spelling whose
    /// final byte it shares with DECSTBM.
    #[test]
    fn the_last_intermediate_byte_is_not_a_value() {
        let params = [
            CsiParam::Integer(1),
            CsiParam::P(b';'),
            CsiParam::Integer(2),
            CsiParam::P(b'/'),
        ];
        let params = CsiParams::parse(&params);
        assert_eq!(params.intermediates(), b"/");
        assert_eq!(params.value(1), Some(2));
    }

    /// Asserts that two intermediate bytes arrive in the order they
    /// were sent.
    ///
    /// Case: an application sends `CSI 1 SP ! p`, a two-intermediate
    /// spelling this terminal answers no control function for.
    #[test]
    fn two_intermediates_arrive_in_order() {
        let params = [CsiParam::Integer(1), CsiParam::P(b' '), CsiParam::P(b'!')];
        let params = CsiParams::parse(&params);
        assert_eq!(params.value(0), Some(1));
        assert_eq!(params.intermediates(), b" !");
    }

    /// Asserts that a colon group stays inside its own slot and does not
    /// shift the slots after it.
    ///
    /// Case: an application sets a curly underline and a colour in one
    /// `CSI 4 : 3 ; 31 m`.
    #[test]
    fn a_colon_group_does_not_shift_later_slots() {
        let params = [
            CsiParam::Integer(4),
            CsiParam::P(b':'),
            CsiParam::Integer(3),
            CsiParam::P(b';'),
            CsiParam::Integer(31),
        ];
        let params = CsiParams::parse(&params);
        assert_eq!(params.value(0), Some(4));
        assert_eq!(params.value(1), Some(31));
    }

    /// Asserts that every separated slot is walked in order.
    ///
    /// Case: an application turns on two private modes at once with
    /// `CSI ? 1 ; 6 h`.
    #[test]
    fn every_slot_is_walked_in_order() {
        let params = [
            CsiParam::P(b'?'),
            CsiParam::Integer(1),
            CsiParam::P(b';'),
            CsiParam::Integer(6),
        ];
        let params = CsiParams::parse(&params);
        assert_eq!(params.values().collect::<Vec<_>>(), vec![Some(1), Some(6)]);
    }

    /// Asserts that an empty sequence yields one empty group.
    ///
    /// Case: an application resets every attribute with the shortest
    /// spelling the standard allows.
    #[test]
    fn an_empty_sequence_yields_one_empty_group() {
        let params = CsiParams::parse(&[]);
        let groups: Vec<_> = params.groups().collect();
        assert_eq!(groups.len(), 1);
        assert!(groups[0].is_empty());
    }

    /// Asserts that a colon group arrives whole, with its separators
    /// and every subparameter still in place.
    ///
    /// Case: an application sets a direct colour with the standard
    /// colon spelling `CSI 38:2::1:2:3 m`.
    #[test]
    fn a_colon_group_arrives_whole() {
        let params = [
            CsiParam::Integer(38),
            CsiParam::P(b':'),
            CsiParam::Integer(2),
            CsiParam::P(b':'),
            CsiParam::P(b':'),
            CsiParam::Integer(1),
            CsiParam::P(b':'),
            CsiParam::Integer(2),
            CsiParam::P(b':'),
            CsiParam::Integer(3),
        ];
        let params = CsiParams::parse(&params);
        let groups: Vec<_> = params.groups().collect();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 10);
    }

    /// Asserts that an omitted slot arrives as an empty group rather
    /// than vanishing.
    ///
    /// Case: an application spells `CSI 1;;31 m`, where the middle slot
    /// is a reset.
    #[test]
    fn an_omitted_slot_is_an_empty_group() {
        let params = [
            CsiParam::Integer(1),
            CsiParam::P(b';'),
            CsiParam::P(b';'),
            CsiParam::Integer(31),
        ];
        let params = CsiParams::parse(&params);
        let groups: Vec<_> = params.groups().collect();
        assert_eq!(groups.len(), 3);
        assert!(groups[1].is_empty());
    }

    /// Asserts that a sequence with no parameters yields no slots.
    ///
    /// Case: an application homes the cursor with a bare `CSI H`.
    #[test]
    fn an_empty_sequence_yields_no_slots() {
        let params = CsiParams::parse(&[]);
        assert_eq!(params.values().count(), 0);
        assert_eq!(params.value(0), None);
    }
}
