//! Single-key matcher that compares `KeyboardEvent` keys against `KeyChord`
//! with case-insensitive matching for single-char keys and exact-case matching
//! for named keys.

import type { KeyChord } from './wire';

const NAMED_KEY_PATTERN = /^[A-Z][a-zA-Z]+$/;

function tokenize(eventKey: string): string {
  return NAMED_KEY_PATTERN.test(eventKey) ? eventKey : eventKey.toLowerCase();
}

function chordToken(chord: KeyChord): string {
  return NAMED_KEY_PATTERN.test(chord.key) ? chord.key : chord.key.toLowerCase();
}

/// Matches a keyboard event against a chord, comparing keys and modifiers.
///
/// Single-character keys are compared case-insensitively to accommodate the
/// fact that `Shift+X` produces `e.key === 'X'` while an unshifted `x` produces
/// `'x'`. Named keys (like `Escape`, `ArrowUp`) are compared exact-case. All
/// modifier flags must match exactly; no partial matches.
export function matchesChord(e: KeyboardEvent, chord: KeyChord): boolean {
  if (tokenize(e.key) !== chordToken(chord)) return false;
  return (
    e.ctrlKey === chord.modifiers.ctrl &&
    e.shiftKey === chord.modifiers.shift &&
    e.altKey === chord.modifiers.alt &&
    e.metaKey === chord.modifiers.meta
  );
}
