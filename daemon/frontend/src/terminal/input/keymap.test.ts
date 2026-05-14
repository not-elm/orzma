import { describe, expect, it } from 'vitest';
import { handleKeyDown } from './keymap';

function ev(init: Partial<KeyboardEventInit> & { key: string }): KeyboardEvent {
  return new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init });
}

describe('handleKeyDown', () => {
  it('returns null during IME composition', () => {
    const e = ev({ key: 'a', isComposing: true });
    expect(handleKeyDown(e, new Set())).toBeNull();
  });

  it('encodes printable ASCII as UTF-8', () => {
    const bytes = handleKeyDown(ev({ key: 'a' }), new Set());
    expect(bytes).toEqual(new Uint8Array([97]));
  });

  it('encodes Enter as 0x0D (CR)', () => {
    expect(handleKeyDown(ev({ key: 'Enter' }), new Set())).toEqual(new Uint8Array([0x0d]));
  });

  it('encodes Backspace as 0x7F (DEL)', () => {
    expect(handleKeyDown(ev({ key: 'Backspace' }), new Set())).toEqual(new Uint8Array([0x7f]));
  });

  it('encodes Tab as 0x09', () => {
    expect(handleKeyDown(ev({ key: 'Tab' }), new Set())).toEqual(new Uint8Array([0x09]));
  });

  it('encodes Escape as 0x1B', () => {
    expect(handleKeyDown(ev({ key: 'Escape' }), new Set())).toEqual(new Uint8Array([0x1b]));
  });

  it('encodes ArrowUp as ESC [ A in normal mode', () => {
    expect(handleKeyDown(ev({ key: 'ArrowUp' }), new Set())).toEqual(
      new TextEncoder().encode('\x1b[A'),
    );
  });

  it('encodes ArrowUp as ESC O A in app-cursor-keys mode', () => {
    expect(handleKeyDown(ev({ key: 'ArrowUp' }), new Set(['app-cursor-keys']))).toEqual(
      new TextEncoder().encode('\x1bOA'),
    );
  });

  it('encodes Ctrl+C as 0x03', () => {
    expect(handleKeyDown(ev({ key: 'c', ctrlKey: true }), new Set())).toEqual(
      new Uint8Array([0x03]),
    );
  });

  it('encodes Ctrl+A as 0x01', () => {
    expect(handleKeyDown(ev({ key: 'a', ctrlKey: true }), new Set())).toEqual(
      new Uint8Array([0x01]),
    );
  });

  it('returns null for modifier-only keydown', () => {
    expect(handleKeyDown(ev({ key: 'Control' }), new Set())).toBeNull();
    expect(handleKeyDown(ev({ key: 'Shift' }), new Set())).toBeNull();
    expect(handleKeyDown(ev({ key: 'Alt' }), new Set())).toBeNull();
    expect(handleKeyDown(ev({ key: 'Meta' }), new Set())).toBeNull();
  });
});
