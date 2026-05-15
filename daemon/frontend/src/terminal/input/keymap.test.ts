import { afterEach, describe, expect, it, vi } from 'vitest';
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

describe('handleKeyDown — clipboard bypass (Phase 3A)', () => {
  const originalPlatform = Object.getOwnPropertyDescriptor(Navigator.prototype, 'platform');

  function setPlatform(p: string): void {
    Object.defineProperty(navigator, 'platform', { value: p, configurable: true });
  }

  afterEach(() => {
    if (originalPlatform) {
      Object.defineProperty(Navigator.prototype, 'platform', originalPlatform);
    }
  });

  it('on macOS: Cmd+V returns null (browser handles paste)', async () => {
    setPlatform('MacIntel');
    vi.resetModules();
    const { handleKeyDown: hk } = await import('./keymap');
    const e = ev({ key: 'v', metaKey: true });
    expect(hk(e, new Set())).toBeNull();
  });

  it('on macOS: Cmd+C returns null', async () => {
    setPlatform('MacIntel');
    vi.resetModules();
    const { handleKeyDown: hk } = await import('./keymap');
    const e = ev({ key: 'c', metaKey: true });
    expect(hk(e, new Set())).toBeNull();
  });

  it('on Linux: bare Ctrl+C still sends ETX 0x03 (SIGINT preserved)', async () => {
    setPlatform('Linux x86_64');
    vi.resetModules();
    const { handleKeyDown: hk } = await import('./keymap');
    const result = hk(ev({ key: 'c', ctrlKey: true }), new Set());
    expect(result).not.toBeNull();
    expect(Array.from(result as Uint8Array)).toEqual([0x03]);
  });

  it('on Linux: bare Ctrl+V still sends 0x16 (^V literal preserved)', async () => {
    setPlatform('Linux x86_64');
    vi.resetModules();
    const { handleKeyDown: hk } = await import('./keymap');
    const result = hk(ev({ key: 'v', ctrlKey: true }), new Set());
    expect(result).not.toBeNull();
    expect(Array.from(result as Uint8Array)).toEqual([0x16]);
  });

  it('on Linux: Ctrl+Shift+V returns null (browser paste)', async () => {
    setPlatform('Linux x86_64');
    vi.resetModules();
    const { handleKeyDown: hk } = await import('./keymap');
    expect(hk(ev({ key: 'V', ctrlKey: true, shiftKey: true }), new Set())).toBeNull();
  });

  it('on Linux: Ctrl+Shift+C returns null (browser copy, no SIGINT regression)', async () => {
    setPlatform('Linux x86_64');
    vi.resetModules();
    const { handleKeyDown: hk } = await import('./keymap');
    expect(hk(ev({ key: 'C', ctrlKey: true, shiftKey: true }), new Set())).toBeNull();
  });
});
