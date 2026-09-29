import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { HeldKeys, type Press } from './held';
import { installKeys, type KeyHandler } from './keys';
import type { Scroller } from './scroller';

type Call =
  | ['scrollBy', number | 'viewSize', number, Press | undefined]
  | ['scrollTo', 'top' | 'bottom', Press | undefined]
  | ['cancelAll'];

const handlers: KeyHandler[] = [];

beforeEach(() => {
  document.body.replaceChildren();
});

afterEach(() => {
  for (const handler of handlers.splice(0)) {
    handler.setEnabled(false);
  }
});

function setup(options: { trusted?: boolean; enabled?: boolean } = {}) {
  const calls: Call[] = [];
  const reports: (string | null)[] = [];
  const scroller: Scroller = {
    scrollBy: (amount, factor, press) => {
      calls.push(['scrollBy', amount, factor, press]);
    },
    scrollTo: (position, press) => {
      calls.push(['scrollTo', position, press]);
    },
    cancelAll: () => {
      calls.push(['cancelAll']);
    },
  };
  const keys = installKeys(window, scroller, new HeldKeys(), {
    reportPending: (key) => {
      reports.push(key);
    },
    isTrusted: options.trusted === false ? undefined : () => true,
  });
  if (options.enabled !== false) {
    keys.setEnabled(true);
  }
  handlers.push(keys);
  return { keys, calls, reports };
}

function send(
  type: 'keydown' | 'keyup',
  init: KeyboardEventInit,
  target: EventTarget = document.body,
): KeyboardEvent {
  const event = new KeyboardEvent(type, {
    bubbles: true,
    cancelable: true,
    composed: true,
    ...init,
  });
  target.dispatchEvent(event);
  return event;
}

function tap(init: KeyboardEventInit, target?: EventTarget): KeyboardEvent {
  const down = send('keydown', init, target);
  send('keyup', init, target);
  return down;
}

function userActs(): void {
  document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
}

function focusedInput(): HTMLInputElement {
  const input = document.createElement('input');
  document.body.append(input);
  input.focus();
  return input;
}

function scrolls(calls: Call[]): unknown[] {
  return calls
    .filter((call) => call[0] !== 'cancelAll')
    .map((call) => call.slice(0, call[0] === 'scrollBy' ? 3 : 2));
}

describe('installKeys', () => {
  it('runs each scroll key', () => {
    const cases: [KeyboardEventInit, unknown[]][] = [
      [{ key: 'j', code: 'KeyJ' }, ['scrollBy', 60, 1]],
      [{ key: 'ArrowDown', code: 'ArrowDown' }, ['scrollBy', 60, 1]],
      [{ key: 'k', code: 'KeyK' }, ['scrollBy', 60, -1]],
      [{ key: 'ArrowUp', code: 'ArrowUp' }, ['scrollBy', 60, -1]],
      [{ key: ' ', code: 'Space' }, ['scrollBy', 'viewSize', 0.5]],
      [{ key: 'd', code: 'KeyD', ctrlKey: true }, ['scrollBy', 'viewSize', 0.5]],
      [{ key: 'u', code: 'KeyU', ctrlKey: true }, ['scrollBy', 'viewSize', -0.5]],
      [{ key: 'f', code: 'KeyF', ctrlKey: true }, ['scrollBy', 'viewSize', 1]],
      [{ key: 'PageDown', code: 'PageDown' }, ['scrollBy', 'viewSize', 1]],
      [{ key: 'b', code: 'KeyB', ctrlKey: true }, ['scrollBy', 'viewSize', -1]],
      [{ key: 'PageUp', code: 'PageUp' }, ['scrollBy', 'viewSize', -1]],
      [{ key: 'G', code: 'KeyG', shiftKey: true }, ['scrollTo', 'bottom']],
    ];
    for (const [init, expected] of cases) {
      const { keys, calls } = setup();
      const down = tap(init);
      expect(scrolls(calls), init.key).toEqual([expected]);
      expect(down.defaultPrevented, init.key).toBe(true);
      keys.setEnabled(false);
    }
  });

  it('leaves keys to the page when off, while typing, or for other chords', () => {
    const { keys, calls } = setup();
    userActs();
    const input = focusedInput();
    const typed = tap({ key: 'j', code: 'KeyJ' }, input);
    input.blur();

    const host = document.createElement('div');
    const shadowInput = document.createElement('input');
    host.attachShadow({ mode: 'open' }).append(shadowInput);
    document.body.append(host);
    shadowInput.focus();
    const typedInShadow = tap({ key: 'j', code: 'KeyJ' }, shadowInput);
    shadowInput.blur();

    const composing = send('keydown', { key: 'j', code: 'KeyJ', isComposing: true });
    const ime = new KeyboardEvent('keydown', {
      key: 'j',
      code: 'KeyJ',
      bubbles: true,
      cancelable: true,
    });
    Object.defineProperty(ime, 'keyCode', { value: 229 });
    document.body.dispatchEvent(ime);
    const others = [
      tap({ key: 'j', code: 'KeyJ', metaKey: true }),
      tap({ key: 'j', code: 'KeyJ', altKey: true }),
      tap({ key: 'J', code: 'KeyJ', shiftKey: true }),
      tap({ key: 'J', code: 'KeyJ' }),
      tap({ key: ' ', code: 'Space', shiftKey: true }),
      tap({ key: 'ArrowDown', code: 'ArrowDown', shiftKey: true }),
      tap({ key: 'x', code: 'KeyX' }),
      tap({ key: 'c', code: 'KeyC', ctrlKey: true }),
    ];
    keys.setEnabled(false);
    const off = tap({ key: 'j', code: 'KeyJ' });
    for (const event of [typed, typedInShadow, composing, ime, ...others, off]) {
      expect(event.defaultPrevented).toBe(false);
    }
    expect(scrolls(calls)).toEqual([]);
  });

  it('ignores key events a page script made', () => {
    const { calls } = setup({ trusted: false });
    expect(tap({ key: 'j', code: 'KeyJ' }).defaultPrevented).toBe(false);
    expect(scrolls(calls)).toEqual([]);
  });

  it('hides a handled key from the page, down and up', () => {
    setup();
    const seen: string[] = [];
    const listen = (event: KeyboardEvent): void => {
      seen.push(`${event.type} ${event.key}`);
    };
    document.addEventListener('keydown', listen);
    document.addEventListener('keyup', listen);
    tap({ key: 'j', code: 'KeyJ' });
    tap({ key: 'x', code: 'KeyX' });
    document.removeEventListener('keydown', listen);
    document.removeEventListener('keyup', listen);
    expect(seen).toEqual(['keydown x', 'keyup x']);
  });

  it('scrolls to the top on gg and reports the pending g', () => {
    const { calls, reports } = setup();
    tap({ key: 'g', code: 'KeyG' });
    expect(reports).toEqual(['g']);
    tap({ key: 'g', code: 'KeyG' });
    expect(reports).toEqual(['g', null]);
    expect(scrolls(calls)).toEqual([['scrollTo', 'top']]);
  });

  it('drops the pending g when another key follows', () => {
    const { calls, reports } = setup();
    tap({ key: 'g', code: 'KeyG' });
    tap({ key: 'j', code: 'KeyJ' });
    expect(reports).toEqual(['g', null]);
    expect(scrolls(calls)).toEqual([['scrollBy', 60, 1]]);

    tap({ key: 'g', code: 'KeyG' });
    const other = tap({ key: 'x', code: 'KeyX' });
    expect(reports).toEqual(['g', null, 'g', null]);
    expect(other.defaultPrevented).toBe(false);
  });

  it('drops the pending g when the user starts typing', () => {
    const { reports } = setup();
    userActs();
    tap({ key: 'g', code: 'KeyG' });
    const input = focusedInput();
    const typed = tap({ key: 'a', code: 'KeyA' }, input);
    expect(reports).toEqual(['g', null]);
    expect(typed.defaultPrevented).toBe(false);
  });

  it('ignores a repeat of the pending g', () => {
    const { calls, reports } = setup();
    send('keydown', { key: 'g', code: 'KeyG' });
    send('keydown', { key: 'g', code: 'KeyG' });
    send('keyup', { key: 'g', code: 'KeyG' });
    expect(reports).toEqual(['g']);
    expect(scrolls(calls)).toEqual([]);
  });

  it('drops the pending g on cancelChord', () => {
    const { keys, reports } = setup();
    tap({ key: 'g', code: 'KeyG' });
    keys.cancelChord();
    expect(reports).toEqual(['g', null]);
  });

  it('starts a fresh chord after the keys go off and on', () => {
    const { keys, calls, reports } = setup();
    tap({ key: 'g', code: 'KeyG' });
    keys.setEnabled(false);
    keys.setEnabled(true);
    tap({ key: 'g', code: 'KeyG' });
    expect(reports).toEqual(['g', null, 'g']);
    expect(scrolls(calls)).toEqual([]);
    expect(calls).toContainEqual(['cancelAll']);
  });

  it('marks a keydown of a held key as a repeat until keyup or blur', () => {
    const { calls } = setup();
    send('keydown', { key: 'j', code: 'KeyJ' });
    send('keydown', { key: 'j', code: 'KeyJ' });
    send('keyup', { key: 'j', code: 'KeyJ' });
    send('keydown', { key: 'j', code: 'KeyJ' });
    window.dispatchEvent(new Event('blur'));
    send('keydown', { key: 'j', code: 'KeyJ' });
    const presses = calls.flatMap((call) => (call[0] === 'scrollBy' && call[3] ? [call[3]] : []));
    expect(presses.map((press) => press.repeat)).toEqual([false, true, false, false]);
    expect(presses[1].id).toBe(presses[0].id);
    expect(presses[2].id).not.toBe(presses[0].id);
  });

  it('stops hiding a key once the window loses focus', () => {
    setup();
    send('keydown', { key: 'j', code: 'KeyJ' });
    window.dispatchEvent(new Event('blur'));
    expect(send('keyup', { key: 'j', code: 'KeyJ' }).defaultPrevented).toBe(false);
  });

  it('takes focus from a text field the page focused before the keys turned on', () => {
    const { keys } = setup({ enabled: false });
    const input = focusedInput();
    keys.setEnabled(true);
    expect(document.activeElement).not.toBe(input);
  });

  it('takes focus from a text field the page focuses before the user acts', () => {
    setup();
    const input = focusedInput();
    expect(document.activeElement).not.toBe(input);
  });

  it('keeps a text field the user focused, even when the keys turn on again', () => {
    const { keys } = setup();
    userActs();
    const input = focusedInput();
    keys.setEnabled(false);
    keys.setEnabled(true);
    expect(document.activeElement).toBe(input);
  });

  it('takes focus from a text field on request', () => {
    const { keys } = setup();
    userActs();
    const input = focusedInput();
    keys.blurFocusedInput();
    expect(document.activeElement).not.toBe(input);
  });

  it('passes a keyup it never saw go down', () => {
    setup();
    let event: KeyboardEvent | undefined;
    expect(() => {
      event = send('keyup', { key: 'j', code: 'KeyJ' });
    }).not.toThrow();
    expect(event?.defaultPrevented).toBe(false);
  });
});
