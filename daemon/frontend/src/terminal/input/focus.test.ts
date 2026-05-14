import { describe, expect, it, vi } from 'vitest';
import { setupFocusEvents } from './focus';

const dec = new TextDecoder();

describe('setupFocusEvents', () => {
  it('does not emit anything when focus-events mode is off', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const send = vi.fn();
    const modesRef = { current: new Set<string>() };
    setupFocusEvents(ta, modesRef, send);

    ta.dispatchEvent(new Event('focus'));
    ta.dispatchEvent(new Event('blur'));

    expect(send).not.toHaveBeenCalled();
    document.body.removeChild(ta);
  });

  it('emits \\e[I on focus when focus-events is set', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const send = vi.fn();
    const modesRef = { current: new Set<string>(['focus-events']) };
    setupFocusEvents(ta, modesRef, send);

    ta.dispatchEvent(new Event('focus'));

    expect(send).toHaveBeenCalledOnce();
    expect(dec.decode(send.mock.calls[0][0] as Uint8Array)).toBe('\x1b[I');
    document.body.removeChild(ta);
  });

  it('emits \\e[O on blur when focus-events is set', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const send = vi.fn();
    const modesRef = { current: new Set<string>(['focus-events']) };
    setupFocusEvents(ta, modesRef, send);

    ta.dispatchEvent(new Event('blur'));

    expect(dec.decode(send.mock.calls[0][0] as Uint8Array)).toBe('\x1b[O');
    document.body.removeChild(ta);
  });

  it('reads modesRef at event time (mid-session mode toggle takes effect)', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const send = vi.fn();
    const modesRef = { current: new Set<string>() };
    setupFocusEvents(ta, modesRef, send);

    ta.dispatchEvent(new Event('focus'));
    expect(send).not.toHaveBeenCalled();

    modesRef.current = new Set(['focus-events']);
    ta.dispatchEvent(new Event('blur'));
    expect(send).toHaveBeenCalledOnce();

    document.body.removeChild(ta);
  });
});
