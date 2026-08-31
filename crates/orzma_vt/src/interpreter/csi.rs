//! The parameter view `csi_dispatch` reads its sequences through.
//!
//! `vtparse` delivers one flat slice that mixes the private marker, the
//! `;`-separated values, and the trailing intermediates. This module is
//! the one place that knows how to tell them apart.

use vtparse::CsiParam;

/// One CSI sequence's parameters, split into the three things vtparse
/// packs into a single slice.
pub(crate) struct CsiParams<'a> {
    private: Option<u8>,
    values: &'a [CsiParam],
    intermediates: &'a [CsiParam],
}

impl<'a> CsiParams<'a> {
    /// Splits one `csi_dispatch` slice into its marker, values, and
    /// intermediates.
    pub(crate) fn parse(params: &'a [CsiParam]) -> Self {
        let (private, rest) = match params.first() {
            Some(CsiParam::P(byte)) if (0x3C..=0x3F).contains(byte) => (Some(*byte), &params[1..]),
            _ => (None, params),
        };
        let split = rest
            .iter()
            .position(|param| matches!(param, CsiParam::P(byte) if (0x20..=0x2F).contains(byte)))
            .unwrap_or(rest.len());
        Self {
            private,
            values: &rest[..split],
            intermediates: &rest[split..],
        }
    }

    /// The private marker a sequence opened with, if it had one.
    pub(crate) fn private(&self) -> Option<u8> {
        self.private
    }

    /// Whether any intermediate byte trails the values.
    ///
    /// This is the guard that separates control functions sharing a
    /// final byte: `vtparse` promotes intermediates into the parameter
    /// slice, so `CSI 1 ; 2 $ r` and `CSI 1 ; 2 r` reach the dispatcher
    /// with the same final byte and differ only here.
    pub(crate) fn has_intermediates(&self) -> bool {
        !self.intermediates.is_empty()
    }

    /// The `;`-separated groups, each carrying its own `:`
    /// subparameters and their separators.
    ///
    /// # Invariants
    ///
    /// An empty sequence yields ONE empty group, not none: a bare
    /// `CSI m` is `SGR 0`, and an empty group is every control
    /// function's own default. [`Self::values`] deliberately reports no
    /// slots for that same input, because a bare `CSI H` listed no
    /// parameters at all — the two views answer different questions.
    pub(crate) fn groups(&self) -> impl Iterator<Item = &'a [CsiParam]> {
        self.values
            .split(|param| matches!(param, CsiParam::P(b';')))
    }

    /// The first value of the `index`-th separated slot; `None` when the
    /// slot was omitted or does not exist.
    ///
    /// A zero reads as `Some(0)`: whether a zero means the default is
    /// each control function's own rule.
    pub(crate) fn value(&self, index: usize) -> Option<u16> {
        self.values().nth(index).flatten()
    }

    /// Every separated slot in order, which is what `SM` and `RM` need
    /// to find the modes they implement among the ones they do not.
    pub(crate) fn values(&self) -> impl Iterator<Item = Option<u16>> + '_ {
        let listed = (!self.values.is_empty()).then(|| self.groups());
        listed.into_iter().flatten().map(Self::first_value)
    }

    /// The saturating `u16` a slot's first integer reads as; `None` for
    /// a slot that carries none.
    ///
    /// # Invariants
    ///
    /// Saturation is right for a slot count or a mode number, where an
    /// oversized value is out of range whichever way it is clamped. A
    /// slot also collapses to one integer here, which loses the `:`
    /// structure a direct colour is spelled with, so `SGR` walks
    /// [`Self::groups`] itself rather than reading them through here.
    fn first_value(group: &[CsiParam]) -> Option<u16> {
        group.iter().find_map(|param| match param {
            CsiParam::Integer(value) => Some(u16::try_from(*value).unwrap_or(u16::MAX)),
            _ => None,
        })
    }
}

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
    /// The agreed policy recognises a marker only in 0x3C..=0x3F. A
    /// looser rule that took whatever punctuation came first would drop
    /// the whole sequence, and `CSI ; Pb r` is a legal DECSTBM spelling.
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
    /// The agreed policy leaves "a zero means the default" to each
    /// control function rather than folding it here, because the rule is
    /// not shared: CUP reads a zero line as line 1, while SGR's zero
    /// means "reset every attribute".
    ///
    /// Case: an application resets its attributes with `CSI 0 m`.
    #[test]
    fn a_zero_is_a_value_and_not_an_omission() {
        let params = [CsiParam::Integer(0)];
        let params = CsiParams::parse(&params);
        assert_eq!(params.value(0), Some(0));
    }

    /// Asserts that a value too large for the parameter type saturates.
    ///
    /// The agreed policy saturates rather than reporting the parameter
    /// as omitted: an omission means "use the default", which would hand
    /// a program asking for an enormous row the ordinary behaviour
    /// instead of the clamped extreme it asked for.
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
    /// sequence `CSI 1 ; 2 $ r`, whose final byte it shares with
    /// DECSTBM.
    #[test]
    fn a_trailing_intermediate_is_not_a_value() {
        let params = [
            CsiParam::Integer(1),
            CsiParam::P(b';'),
            CsiParam::Integer(2),
            CsiParam::P(b'$'),
        ];
        let params = CsiParams::parse(&params);
        assert!(params.has_intermediates());
        assert_eq!(params.value(0), Some(1));
        assert_eq!(params.value(1), Some(2));
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

    /// Asserts that an empty sequence yields one empty group, which is
    /// how a bare `CSI m` reads as `SGR 0`.
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
