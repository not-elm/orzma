//! The kitty keyboard protocol's progressive-enhancement flags and the per-screen stack of them.

use bitflags::bitflags;
use std::collections::VecDeque;

bitflags! {
    /// The progressive-enhancement flags of the kitty keyboard protocol.
    ///
    /// # References
    ///
    /// <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct KittyKeyboardFlags: u16 {
        /// The "Disambiguate escape codes" enhancement.
        const DISAMBIGUATE = 1;
        /// The "Report event types" enhancement: key press, repeat and release events.
        const REPORT_EVENT_TYPES = 1 << 1;
        /// The "Report alternate keys" enhancement.
        const REPORT_ALTERNATE_KEYS = 1 << 2;
        /// The "Report all keys as escape codes" enhancement.
        const REPORT_ALL_KEYS = 1 << 3;
        /// The "Report associated text" enhancement: the text a key produces.
        const REPORT_ASSOCIATED_TEXT = 1 << 4;
    }
}

impl KittyKeyboardFlags {
    /// The flags this terminal implements; a request's other bits are dropped.
    ///
    /// TODO: add each flag as it is implemented.
    pub const SUPPORTED: Self = Self::DISAMBIGUATE;

    /// Keeps only the [`SUPPORTED`](Self::SUPPORTED) bits of a request; a value past `u16` keeps
    /// its supported low bits rather than saturating.
    pub fn from_request(raw: i64) -> Self {
        let masked = raw & i64::from(Self::SUPPORTED.bits());
        Self::from_bits_truncate(u16::try_from(masked).unwrap_or(0))
    }
}

/// How a flag-set request combines its flags with the top stack entry.
///
/// # References
///
/// <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardFlagSetMode {
    /// The request's flags replace the entry.
    Replace,
    /// The request's flags are added to the entry.
    Add,
    /// The request's flags are removed from the entry.
    Remove,
}

impl KeyboardFlagSetMode {
    /// Maps a request's mode parameter: 1, 2 and 3 select a mode, any other value is `None`.
    pub fn from_param(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::Replace),
            2 => Some(Self::Add),
            3 => Some(Self::Remove),
            _ => None,
        }
    }
}

/// The stack of keyboard flag sets one screen keeps; the top entry is in force.
///
/// # Invariants
///
/// The stack never holds more than [`CAPACITY`](Self::CAPACITY) entries.
///
/// # References
///
/// <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>
#[derive(Debug, Default)]
pub struct KittyKeyboardStack(VecDeque<KittyKeyboardFlags>);

impl KittyKeyboardStack {
    /// The most entries the stack holds.
    pub const CAPACITY: usize = 8;

    /// The flags in force: the top entry, or no flags when the stack is empty.
    pub fn current(&self) -> KittyKeyboardFlags {
        self.0.back().copied().unwrap_or_default()
    }

    /// Pushes an entry, evicting the oldest one when the stack is full.
    pub fn push(&mut self, flags: KittyKeyboardFlags) {
        if self.0.len() >= Self::CAPACITY {
            self.0.pop_front();
        }
        self.0.push_back(flags);
    }

    /// Pops up to `count` entries; a zero count or an empty stack pops nothing.
    pub fn pop(&mut self, count: u16) {
        let keep = self.0.len().saturating_sub(usize::from(count));
        self.0.truncate(keep);
    }

    /// Rewrites the top entry by `mode`, creating an entry first when the stack is empty.
    pub fn set(&mut self, flags: KittyKeyboardFlags, mode: KeyboardFlagSetMode) {
        let top = self.0.pop_back().unwrap_or_default();
        self.0.push_back(match mode {
            KeyboardFlagSetMode::Replace => flags,
            KeyboardFlagSetMode::Add => top.union(flags),
            KeyboardFlagSetMode::Remove => top.difference(flags),
        });
    }

    /// Drops every entry.
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that pushing onto a full stack evicts the oldest entry.
    ///
    /// Case: an application pushes nine flag sets without popping any of them.
    #[test]
    fn a_full_stack_evicts_its_oldest_entry() {
        let mut stack = KittyKeyboardStack::default();
        stack.push(KittyKeyboardFlags::DISAMBIGUATE);
        for _ in 0..8 {
            stack.push(KittyKeyboardFlags::REPORT_EVENT_TYPES);
        }
        stack.pop(7);
        assert_eq!(stack.current(), KittyKeyboardFlags::REPORT_EVENT_TYPES);
        stack.pop(1);
        assert_eq!(stack.current(), KittyKeyboardFlags::empty());
    }

    /// Asserts that popping zero entries leaves the stack unchanged.
    ///
    /// Case: an application sends a pop request with a zero count.
    #[test]
    fn a_pop_of_zero_entries_changes_nothing() {
        let mut stack = KittyKeyboardStack::default();
        stack.push(KittyKeyboardFlags::DISAMBIGUATE);
        stack.pop(0);
        assert_eq!(stack.current(), KittyKeyboardFlags::DISAMBIGUATE);
    }

    /// Asserts that a pop on an empty stack does nothing.
    ///
    /// Case: an application pops more often than it pushed.
    #[test]
    fn a_pop_on_an_empty_stack_does_nothing() {
        let mut stack = KittyKeyboardStack::default();
        stack.pop(3);
        assert_eq!(stack.current(), KittyKeyboardFlags::empty());
    }

    /// Asserts that replace, add and remove each rewrite the top entry.
    ///
    /// Case: an application adjusts its enhancement flags in place after pushing a set.
    #[test]
    fn the_three_set_modes_rewrite_the_top_entry() {
        let mut stack = KittyKeyboardStack::default();
        stack.push(KittyKeyboardFlags::DISAMBIGUATE);
        stack.set(
            KittyKeyboardFlags::REPORT_EVENT_TYPES,
            KeyboardFlagSetMode::Add,
        );
        assert_eq!(
            stack.current(),
            KittyKeyboardFlags::DISAMBIGUATE | KittyKeyboardFlags::REPORT_EVENT_TYPES
        );
        stack.set(
            KittyKeyboardFlags::DISAMBIGUATE,
            KeyboardFlagSetMode::Remove,
        );
        assert_eq!(stack.current(), KittyKeyboardFlags::REPORT_EVENT_TYPES);
        stack.set(
            KittyKeyboardFlags::DISAMBIGUATE,
            KeyboardFlagSetMode::Replace,
        );
        assert_eq!(stack.current(), KittyKeyboardFlags::DISAMBIGUATE);
    }

    /// Asserts that a set on an empty stack creates one entry.
    ///
    /// Case: an application enables the flags without ever pushing.
    #[test]
    fn a_set_on_an_empty_stack_creates_one_entry() {
        let mut stack = KittyKeyboardStack::default();
        stack.set(
            KittyKeyboardFlags::DISAMBIGUATE,
            KeyboardFlagSetMode::Replace,
        );
        assert_eq!(stack.current(), KittyKeyboardFlags::DISAMBIGUATE);
        stack.pop(1);
        assert_eq!(stack.current(), KittyKeyboardFlags::empty());
    }

    /// Asserts that a request keeps only the supported bits, including past `u16`.
    ///
    /// Case: an application requests flags this terminal does not implement, or an oversized value.
    #[test]
    fn a_request_keeps_only_the_supported_bits() {
        assert_eq!(
            KittyKeyboardFlags::from_request(5),
            KittyKeyboardFlags::DISAMBIGUATE
        );
        assert_eq!(
            KittyKeyboardFlags::from_request(30),
            KittyKeyboardFlags::empty()
        );
        assert_eq!(
            KittyKeyboardFlags::from_request(65536),
            KittyKeyboardFlags::empty()
        );
        assert_eq!(
            KittyKeyboardFlags::from_request(65537),
            KittyKeyboardFlags::DISAMBIGUATE
        );
    }

    /// Asserts that only modes one to three are set modes.
    ///
    /// Case: an application sends a flag-set request with an out-of-range mode parameter.
    #[test]
    fn only_modes_one_to_three_are_set_modes() {
        assert_eq!(
            KeyboardFlagSetMode::from_param(1),
            Some(KeyboardFlagSetMode::Replace)
        );
        assert_eq!(
            KeyboardFlagSetMode::from_param(2),
            Some(KeyboardFlagSetMode::Add)
        );
        assert_eq!(
            KeyboardFlagSetMode::from_param(3),
            Some(KeyboardFlagSetMode::Remove)
        );
        assert_eq!(KeyboardFlagSetMode::from_param(0), None);
        assert_eq!(KeyboardFlagSetMode::from_param(4), None);
    }
}
