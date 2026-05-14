import { describe, expect, it, vi } from 'vitest';
import { encodePaste, setupPaste } from './paste';

const dec = new TextDecoder();

describe('encodePaste', () => {
  it('normalizes \\r\\n to \\r without bracketed mode', () => {
    const bytes = encodePaste('a\nb\r\nc', new Set());
    expect(dec.decode(bytes)).toBe('a\rb\rc');
  });

  it('normalizes and wraps under bracketed-paste mode', () => {
    const bytes = encodePaste('hi\nthere', new Set(['bracketed-paste']));
    expect(dec.decode(bytes)).toBe('\x1b[200~hi\rthere\x1b[201~');
  });

  it('sanitizes ESC to U+241B inside bracketed wrapper', () => {
    const bytes = encodePaste('foo\x1bbar', new Set(['bracketed-paste']));
    expect(dec.decode(bytes)).toBe('\x1b[200~foo␛bar\x1b[201~');
  });

  it('does NOT sanitize ESC outside bracketed mode', () => {
    const bytes = encodePaste('foo\x1bbar', new Set());
    expect(dec.decode(bytes)).toBe('foo\x1bbar');
  });
});

// NOTE: jsdom lacks DataTransfer; use a fake clipboardData via defineProperty.
function firePaste(ta: HTMLTextAreaElement, text: string | null): void {
  const fakeClipboard = text === null ? null : { getData: (_t: string) => text };
  const e = new Event('paste', { cancelable: true });
  Object.defineProperty(e, 'clipboardData', { value: fakeClipboard });
  ta.dispatchEvent(e);
}

describe('setupPaste', () => {
  it('skips when clipboardData is null', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const send = vi.fn();
    const modesRef = { current: new Set<string>() };
    setupPaste(ta, modesRef, send);

    firePaste(ta, null);

    expect(send).not.toHaveBeenCalled();
    document.body.removeChild(ta);
  });

  it('skips when clipboardData.getData returns empty string', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const send = vi.fn();
    const modesRef = { current: new Set<string>() };
    setupPaste(ta, modesRef, send);

    firePaste(ta, '');

    expect(send).not.toHaveBeenCalled();
    document.body.removeChild(ta);
  });

  it('dispatches CR-normalized bracketed bytes when text is present', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const send = vi.fn();
    const modesRef = { current: new Set<string>(['bracketed-paste']) };
    setupPaste(ta, modesRef, send);

    firePaste(ta, 'one\ntwo');

    expect(send).toHaveBeenCalledOnce();
    expect(dec.decode(send.mock.calls[0][0] as Uint8Array)).toBe('\x1b[200~one\rtwo\x1b[201~');
    document.body.removeChild(ta);
  });

  it('dispatches raw bytes when bracketed mode is off', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const send = vi.fn();
    const modesRef = { current: new Set<string>() };
    setupPaste(ta, modesRef, send);

    firePaste(ta, 'plain text');

    expect(send).toHaveBeenCalledOnce();
    expect(dec.decode(send.mock.calls[0][0] as Uint8Array)).toBe('plain text');
    document.body.removeChild(ta);
  });

  it('cleanup removes paste listener', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const send = vi.fn();
    const modesRef = { current: new Set<string>() };
    const cleanup = setupPaste(ta, modesRef, send);

    cleanup();
    firePaste(ta, 'should not fire');

    expect(send).not.toHaveBeenCalled();
    document.body.removeChild(ta);
  });
});
