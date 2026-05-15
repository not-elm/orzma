// Tests that the global prefix dispatcher intercepts Ctrl+B before the
// keyboard wire handler sees it, locking in the capture-phase install invariant.

import { afterEach, describe, expect, it } from 'vitest';
import { createKeyDispatcher } from '../shortcuts/globalKeyDispatcher';
import { attachKeyboard } from './input/keyboard';
import type { BrowserClientMsg } from './protocol/wire';

afterEach(() => {
  document.body.innerHTML = '';
});

// Ctrl modifier bitmask per the CDP convention used by modBits() (Alt=1, Ctrl=2, Meta=4, Shift=8).
const CTRL_BIT = 2;

function makeTextarea(): HTMLTextAreaElement {
  const ta = document.createElement('textarea');
  document.body.appendChild(ta);
  return ta;
}

function collectSent(ta: HTMLTextAreaElement): { sent: BrowserClientMsg[]; detach: () => void } {
  const sent: BrowserClientMsg[] = [];
  const detach = attachKeyboard(ta, (m) => sent.push(m));
  return { sent, detach };
}

describe('BrowserActivity keyboard wiring', () => {
  it('Ctrl+B is intercepted by globalKeyDispatcher (capture phase) and NOT forwarded as a key message', () => {
    const ta = makeTextarea();
    const { sent, detach } = collectSent(ta);

    // Install the global dispatcher in capture phase: it intercepts Ctrl+B.
    const dispatcher = createKeyDispatcher((e: KeyboardEvent) => {
      if (e.ctrlKey && e.key.toLowerCase() === 'b') {
        e.stopPropagation();
        e.preventDefault();
      }
    });
    dispatcher.attachTo(ta);

    ta.focus();
    ta.dispatchEvent(
      new KeyboardEvent('keydown', {
        key: 'b',
        code: 'KeyB',
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );

    // The capture-phase handler stopped propagation, so the bubble-phase
    // keyboard handler must not have produced any key message for Ctrl+B.
    const ctrlB = sent.find(
      (m) => m.kind === 'key' && m.code === 'KeyB' && (m.modifiers & CTRL_BIT) !== 0,
    );
    expect(ctrlB).toBeUndefined();

    detach();
    dispatcher.detachFrom(ta);
  });

  it('a non-prefix key (KeyA) IS forwarded as a key message', () => {
    const ta = makeTextarea();
    const { sent, detach } = collectSent(ta);

    // Even with the global dispatcher installed, a plain 'a' key must pass through.
    const dispatcher = createKeyDispatcher((e: KeyboardEvent) => {
      if (e.ctrlKey && e.key.toLowerCase() === 'b') {
        e.stopPropagation();
        e.preventDefault();
      }
    });
    dispatcher.attachTo(ta);

    ta.dispatchEvent(
      new KeyboardEvent('keydown', {
        key: 'a',
        code: 'KeyA',
        bubbles: true,
        cancelable: true,
      }),
    );

    const keyA = sent.find((m) => m.kind === 'key' && m.code === 'KeyA');
    expect(keyA).toBeDefined();
    expect(keyA?.kind === 'key' && keyA.text).toBe('a');

    detach();
    dispatcher.detachFrom(ta);
  });

  it('Ctrl+B without the dispatcher installed IS forwarded (confirms the dispatcher is what blocks it)', () => {
    const ta = makeTextarea();
    const { sent, detach } = collectSent(ta);

    // No dispatcher — the key falls through to the bubble-phase handler.
    ta.dispatchEvent(
      new KeyboardEvent('keydown', {
        key: 'b',
        code: 'KeyB',
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );

    const ctrlB = sent.find(
      (m) => m.kind === 'key' && m.code === 'KeyB' && (m.modifiers & CTRL_BIT) !== 0,
    );
    expect(ctrlB).toBeDefined();

    detach();
  });
});
