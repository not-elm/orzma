import { describe, expect, it } from 'vitest';
import { matchesChord } from './chord';
import type { KeyChord } from './wire';

function ev(init: KeyboardEventInit & { key: string }): KeyboardEvent {
  return new KeyboardEvent('keydown', { bubbles: true, ...init });
}

const NO_MODS = { ctrl: false, shift: false, alt: false, meta: false };

describe('matchesChord', () => {
  it('matches a single-char key case-insensitively when no modifiers', () => {
    const chord: KeyChord = { key: 'x', modifiers: NO_MODS };
    expect(matchesChord(ev({ key: 'x' }), chord)).toBe(true);
    expect(matchesChord(ev({ key: 'X' }), chord)).toBe(true);
  });

  it('requires every modifier flag to match exactly', () => {
    const chord: KeyChord = {
      key: 'x',
      modifiers: { ctrl: true, shift: false, alt: false, meta: false },
    };
    expect(matchesChord(ev({ key: 'x', ctrlKey: true }), chord)).toBe(true);
    // missing ctrl
    expect(matchesChord(ev({ key: 'x' }), chord)).toBe(false);
    // extra modifier
    expect(matchesChord(ev({ key: 'x', ctrlKey: true, shiftKey: true }), chord)).toBe(false);
  });

  it('matches named keys exactly on the token', () => {
    const chord: KeyChord = { key: 'Escape', modifiers: NO_MODS };
    expect(matchesChord(ev({ key: 'Escape' }), chord)).toBe(true);
    expect(matchesChord(ev({ key: 'escape' }), chord)).toBe(false);
  });

  it('does not match different keys', () => {
    const chord: KeyChord = { key: 'x', modifiers: NO_MODS };
    expect(matchesChord(ev({ key: 'q' }), chord)).toBe(false);
  });
});
