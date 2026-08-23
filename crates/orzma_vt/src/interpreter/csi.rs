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

    /// The first value of the `index`-th separated slot; `None` when the
    /// slot was omitted or does not exist.
    ///
    /// A zero reads as `Some(0)`: whether a zero means the default is
    /// each control function's own rule.
    pub(crate) fn value(&self, index: usize) -> Option<u16> {
        let mut slot = 0;
        for param in self.values {
            match param {
                CsiParam::P(b';') => {
                    if slot == index {
                        return None;
                    }
                    slot += 1;
                }
                CsiParam::Integer(value) if slot == index => {
                    return Some(u16::try_from(*value).unwrap_or(u16::MAX));
                }
                _ => {}
            }
        }
        None
    }

    /// Every separated slot in order, which is what `SM` and `RM` need
    /// to find the modes they implement among the ones they do not.
    pub(crate) fn values(&self) -> impl Iterator<Item = Option<u16>> + '_ {
        (0..self.slot_count()).map(|index| self.value(index))
    }

    fn slot_count(&self) -> usize {
        if self.values.is_empty() {
            return 0;
        }
        1 + self
            .values
            .iter()
            .filter(|param| matches!(param, CsiParam::P(b';')))
            .count()
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
