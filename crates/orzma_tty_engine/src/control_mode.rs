//! tmux `-CC` control-mode handover detection for watched terminals.
//!
//! When a Default-mode shell runs `tmux -CC`, tmux emits the DCS introducer
//! `ESC P 1000 p` then switches to its control-mode protocol stream. A
//! terminal carrying a [`ControlModeWatch`] component has its inbound PTY
//! bytes scanned for that introducer ([`Handover::scan`]): the launching
//! shell line is advanced into the VT cleanly, then VT feeding stops and the
//! introducer-onward bytes are buffered raw on [`AdoptedControlMode`] for a
//! later task to drive the tmux protocol.

use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use std::mem::take;

/// The tmux control-mode DCS introducer: `ESC P 1000 p`.
const CONTROL_MODE_INTRODUCER: &[u8] = b"\x1bP1000p";

/// Per-terminal handshake-watch state.
///
/// Holds a carry of bytes that form a strict (proper) prefix of the
/// introducer and were withheld from the VT pending resolution across a
/// PTY chunk boundary. Empty in the steady state.
#[derive(Component, Default)]
pub struct ControlModeWatch {
    carry: Vec<u8>,
}

/// Post-introducer raw bytes buffered for the in-world tmux protocol drive.
///
/// Present once the handover has fired; the watched terminal's PTY bytes are
/// appended here verbatim instead of being fed to the VT.
#[derive(Component, Default)]
pub struct AdoptedControlMode {
    pub(crate) captured: Vec<u8>,
}

impl AdoptedControlMode {
    /// Returns a component pre-seeded with `captured` post-introducer bytes.
    ///
    /// Lets consumers of the in-world drive (and their tests) stage captured
    /// bytes without reaching the private buffer field.
    pub fn from_captured(captured: Vec<u8>) -> Self {
        Self { captured }
    }

    /// Removes and returns the buffered bytes, leaving the buffer empty.
    ///
    /// The returned slice begins at the introducer byte; downstream
    /// `ProtocolClient::feed` strips the introducer before parsing.
    pub fn take_captured(&mut self) -> Vec<u8> {
        take(&mut self.captured)
    }
}

/// Fired once on the terminal entity when the control-mode introducer is
/// detected in its inbound PTY stream.
#[derive(EntityEvent)]
pub struct ControlModeDetected {
    /// The terminal entity whose stream entered control mode.
    #[event_target]
    pub entity: Entity,
}

/// Outcome of feeding one PTY chunk through the handover scanner.
pub(crate) enum Handover {
    /// No (complete) introducer yet. Feed `vt` to the VT; the scanner has
    /// retained any trailing partial-introducer bytes internally (NOT in
    /// `vt`) on `watch.carry`.
    NotYet { vt: Vec<u8> },
    /// Introducer found. Feed `vt` (pre-introducer bytes) to the VT, then
    /// enter capture mode seeded with `captured` (introducer byte onward).
    Detected { vt: Vec<u8>, captured: Vec<u8> },
}

impl Handover {
    /// Scans `chunk` (joined with any carried partial-introducer prefix) for the
    /// control-mode introducer.
    ///
    /// Bytes that may begin an introducer are withheld from `vt` and carried to
    /// the next call, so the VT never sees a partial introducer; a disproven
    /// match flushes the carried bytes into a later `vt`.
    pub(crate) fn scan(watch: &mut ControlModeWatch, chunk: &[u8]) -> Self {
        let mut working = take(&mut watch.carry);
        working.extend_from_slice(chunk);

        if let Some(p) = find_introducer(&working) {
            return Self::Detected {
                vt: working[..p].to_vec(),
                captured: working[p..].to_vec(),
            };
        }

        let s = longest_proper_prefix_suffix(&working);
        let split = working.len() - s;
        watch.carry = working[split..].to_vec();
        working.truncate(split);
        Self::NotYet { vt: working }
    }
}

/// Returns the index of the first full [`CONTROL_MODE_INTRODUCER`]
/// occurrence in `haystack`, or `None`. Hand-rolled over the fixed 7-byte
/// needle to avoid a `memchr` dependency.
fn find_introducer(haystack: &[u8]) -> Option<usize> {
    let needle = CONTROL_MODE_INTRODUCER;
    if haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .find(|&start| &haystack[start..start + needle.len()] == needle)
}

/// Returns the length of the longest suffix of `working` that is also a
/// proper prefix (length `1..needle.len()`) of [`CONTROL_MODE_INTRODUCER`].
///
/// Used to decide how many trailing bytes to withhold from the VT as a
/// pending partial-introducer match. Returns `0` when no such suffix exists.
fn longest_proper_prefix_suffix(working: &[u8]) -> usize {
    let needle = CONTROL_MODE_INTRODUCER;
    let max = (needle.len() - 1).min(working.len());
    (1..=max)
        .rev()
        .find(|&len| working[working.len() - len..] == needle[..len])
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(watch: &mut ControlModeWatch, chunk: &[u8]) -> Handover {
        Handover::scan(watch, chunk)
    }

    fn assert_detected(handover: Handover, expected_vt: &[u8], expected_captured: &[u8]) {
        match handover {
            Handover::Detected { vt, captured } => {
                assert_eq!(vt, expected_vt, "Detected vt mismatch");
                assert_eq!(captured, expected_captured, "Detected captured mismatch");
            }
            Handover::NotYet { vt } => {
                panic!("expected Detected, got NotYet {{ vt: {vt:?} }}");
            }
        }
    }

    fn assert_not_yet(handover: Handover, expected_vt: &[u8]) {
        match handover {
            Handover::NotYet { vt } => assert_eq!(vt, expected_vt, "NotYet vt mismatch"),
            Handover::Detected { vt, captured } => {
                panic!("expected NotYet, got Detected {{ vt: {vt:?}, captured: {captured:?} }}");
            }
        }
    }

    #[test]
    fn scan_finds_introducer_at_start() {
        let mut w = ControlModeWatch::default();
        assert_detected(scan(&mut w, b"\x1bP1000p%begin"), b"", b"\x1bP1000p%begin");
        assert!(w.carry.is_empty());
    }

    #[test]
    fn scan_finds_introducer_midchunk() {
        let mut w = ControlModeWatch::default();
        assert_detected(
            scan(&mut w, b"$ tmux -CC\r\n\x1bP1000p"),
            b"$ tmux -CC\r\n",
            b"\x1bP1000p",
        );
        assert!(w.carry.is_empty());
    }

    #[test]
    fn scan_handles_introducer_split_across_chunks() {
        let mut w = ControlModeWatch::default();
        assert_not_yet(scan(&mut w, b"out\x1bP10"), b"out");
        assert_eq!(
            w.carry, b"\x1bP10",
            "carry must hold the partial introducer"
        );
        assert_detected(scan(&mut w, b"00p%begin"), b"", b"\x1bP1000p%begin");
        assert!(w.carry.is_empty());
    }

    #[test]
    fn scan_false_alarm_flushes_carry() {
        let mut w = ControlModeWatch::default();
        assert_not_yet(scan(&mut w, b"out\x1b"), b"out");
        assert_eq!(w.carry, b"\x1b", "lone ESC is a 1-byte carry");
        assert_not_yet(scan(&mut w, b"X y"), b"\x1bX y");
        assert!(w.carry.is_empty(), "disproven match flushes the carry");
    }

    #[test]
    fn scan_no_esc_passes_through() {
        let mut w = ControlModeWatch::default();
        assert_not_yet(scan(&mut w, b"normal"), b"normal");
        assert!(w.carry.is_empty());
    }

    #[test]
    fn take_captured_drains_buffer() {
        let mut adopted = AdoptedControlMode {
            captured: b"\x1bP1000p%begin".to_vec(),
        };
        assert_eq!(adopted.take_captured(), b"\x1bP1000p%begin");
        assert!(adopted.take_captured().is_empty());
    }
}
