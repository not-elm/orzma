import { HeldKeys, type Press } from '@orzma/scroller';
import { beforeEach, describe, expect, it } from 'vitest';
import type { SearchStage } from './find';
import { installKeys, type RelayedKey } from './keys';

beforeEach(() => {
  document.body.replaceChildren();
});

const J = { key: 'j', code: 'KeyJ' };
const G = { key: 'g', code: 'KeyG' };
const Q = { key: 'q', code: 'KeyQ' };
const R = { key: 'r', code: 'KeyR' };
const SLASH = { key: '/', code: 'Slash' };
const ENTER = { key: 'Enter', code: 'Enter' };
const ESC = { key: 'Escape', code: 'Escape' };

function label(press: Press | undefined): string {
  if (press === undefined) {
    return 'tap';
  }
  return press.repeat ? 'repeat' : 'press';
}

function setup(
  options: {
    headings?: number;
    current?: number | null;
    stage?: SearchStage;
    outline?: boolean;
  } = {},
) {
  const log: string[] = [];
  let toastsDismissed = 0;
  let outlineOpen = options.outline ?? false;
  const win = Object.assign(new EventTarget(), { document });
  const find = {
    stage: options.stage ?? ('closed' as SearchStage),
    open: () => {
      find.stage = 'typing';
      log.push('find.open');
    },
    typeText: (text: string) => {
      log.push(`find.type:${text}`);
    },
    backspace: () => {
      log.push('find.backspace');
    },
    enter: () => {
      log.push('find.enter');
    },
    escape: () => {
      log.push('find.escape');
    },
    nav: (dir: 'next' | 'prev') => {
      log.push(`find.nav:${dir}`);
    },
    clearHighlights: () => {
      log.push('find.clear');
    },
  };
  const relay = installKeys(win, new HeldKeys(), {
    scroller: {
      scrollBy: (amount, factor, press) => {
        log.push(`scrollBy:${amount}:${factor}:${label(press)}`);
        return true;
      },
      scrollTo: (position, press) => {
        log.push(`scrollTo:${position}:${label(press)}`);
        return true;
      },
      cancelAll: () => {
        log.push('cancelAll');
      },
    },
    outline: {
      isOpen: () => outlineOpen,
      open: (current) => {
        outlineOpen = true;
        log.push(`outline.open:${current}`);
      },
      close: () => {
        outlineOpen = false;
        log.push('outline.close');
      },
      move: (delta) => {
        log.push(`outline.move:${delta}`);
      },
      choose: () => {
        log.push('outline.choose');
      },
    },
    find,
    jumpToHeading: (index) => {
      log.push(`jump:${index}`);
    },
    currentHeading: () => options.current ?? null,
    headingCount: () => options.headings ?? 0,
    showPending: (key) => {
      log.push(`pending:${key}`);
    },
    dismissToast: () => {
      toastsDismissed += 1;
    },
    request: (kind) => {
      log.push(`request:${kind}`);
    },
  });
  const send = (type: 'keydown' | 'keyup', init: KeyboardEventInit): KeyboardEvent => {
    const event = new KeyboardEvent(type, { cancelable: true, ...init });
    win.dispatchEvent(event);
    return event;
  };
  const tap = (init: KeyboardEventInit): KeyboardEvent => {
    const down = send('keydown', init);
    send('keyup', init);
    return down;
  };
  return {
    log,
    win,
    send,
    tap,
    relay: (key: Partial<RelayedKey> & { key: string }) =>
      relay({ ctrl: false, alt: false, shift: false, ...key }),
    dismissed: () => toastsDismissed,
  };
}

function focusInput(readOnly = false): HTMLInputElement {
  const input = document.createElement('input');
  input.readOnly = readOnly;
  document.body.append(input);
  input.focus();
  return input;
}

describe('installKeys', () => {
  it('runs each reading key as a held scroll', () => {
    const cases: [KeyboardEventInit, string][] = [
      [J, 'scrollBy:60:1:press'],
      [{ key: 'ArrowDown', code: 'ArrowDown' }, 'scrollBy:60:1:press'],
      [{ key: 'k', code: 'KeyK' }, 'scrollBy:60:-1:press'],
      [{ key: 'ArrowUp', code: 'ArrowUp' }, 'scrollBy:60:-1:press'],
      [{ key: 'd', code: 'KeyD', ctrlKey: true }, 'scrollBy:viewSize:0.5:press'],
      [{ key: 'u', code: 'KeyU', ctrlKey: true }, 'scrollBy:viewSize:-0.5:press'],
      [{ key: 'f', code: 'KeyF', ctrlKey: true }, 'scrollBy:viewSize:1:press'],
      [{ key: ' ', code: 'Space' }, 'scrollBy:viewSize:1:press'],
      [{ key: 'PageDown', code: 'PageDown' }, 'scrollBy:viewSize:1:press'],
      [{ key: 'b', code: 'KeyB', ctrlKey: true }, 'scrollBy:viewSize:-1:press'],
      [{ key: 'PageUp', code: 'PageUp' }, 'scrollBy:viewSize:-1:press'],
      [{ key: 'G', code: 'KeyG', shiftKey: true }, 'scrollTo:bottom:press'],
    ];
    for (const [init, expected] of cases) {
      const t = setup();
      const down = t.tap(init);
      expect(t.log, String(init.key)).toEqual([expected]);
      expect(down.defaultPrevented, String(init.key)).toBe(true);
    }
  });

  it('scrolls to the top on gg and shows the pending g in between', () => {
    const t = setup();
    t.tap(G);
    expect(t.log).toEqual(['pending:g']);
    t.tap(G);
    expect(t.log).toEqual(['pending:g', 'pending:null', 'scrollTo:top:press']);
  });

  it('jumps to the next and the previous heading on ]] and [[', () => {
    const next = setup({ headings: 3, current: 0 });
    next.tap({ key: ']', code: 'BracketRight' });
    next.tap({ key: ']', code: 'BracketRight' });
    expect(next.log).toEqual(['pending:]', 'pending:null', 'jump:1']);

    const prev = setup({ headings: 3, current: 2 });
    prev.tap({ key: '[', code: 'BracketLeft' });
    prev.tap({ key: '[', code: 'BracketLeft' });
    expect(prev.log).toEqual(['pending:[', 'pending:null', 'jump:1']);
  });

  it('jumps to the first heading from above it and never past the ends', () => {
    const above = setup({ headings: 3, current: null });
    above.tap({ key: ']', code: 'BracketRight' });
    above.tap({ key: ']', code: 'BracketRight' });
    expect(above.log).toEqual(['pending:]', 'pending:null', 'jump:0']);

    const last = setup({ headings: 3, current: 2 });
    last.tap({ key: ']', code: 'BracketRight' });
    last.tap({ key: ']', code: 'BracketRight' });
    expect(last.log).toEqual(['pending:]', 'pending:null']);

    const none = setup({ headings: 0 });
    none.tap({ key: '[', code: 'BracketLeft' });
    none.tap({ key: '[', code: 'BracketLeft' });
    expect(none.log).toEqual(['pending:[', 'pending:null']);
  });

  it('drops a pending chord on another key and runs that key', () => {
    const t = setup();
    t.tap(G);
    t.tap(J);
    expect(t.log).toEqual(['pending:g', 'pending:null', 'scrollBy:60:1:press']);
  });

  it('ignores the repeats of a held chord key', () => {
    const t = setup();
    t.send('keydown', G);
    t.send('keydown', G);
    expect(t.log).toEqual(['pending:g']);
    t.send('keyup', G);
    t.tap(G);
    expect(t.log).toEqual(['pending:g', 'pending:null', 'scrollTo:top:press']);
  });

  it('passes the repeats of a held scroll key on as repeats', () => {
    const t = setup();
    t.send('keydown', J);
    t.send('keydown', J);
    expect(t.log).toEqual(['scrollBy:60:1:press', 'scrollBy:60:1:repeat']);
  });

  it('moves between matches on n and N', () => {
    const t = setup({ stage: 'active' });
    t.tap({ key: 'n', code: 'KeyN' });
    t.tap({ key: 'N', code: 'KeyN', shiftKey: true });
    expect(t.log).toEqual(['find.nav:next', 'find.nav:prev']);
  });

  it('opens the outline on the heading being read with o and Tab', () => {
    for (const init of [
      { key: 'o', code: 'KeyO' },
      { key: 'Tab', code: 'Tab' },
    ]) {
      const t = setup({ headings: 3, current: 2 });
      const down = t.tap(init);
      expect(t.log, init.key).toEqual(['outline.open:2']);
      expect(down.defaultPrevented, init.key).toBe(true);
    }
  });

  it('drives the open outline and swallows the other reading keys', () => {
    const t = setup({ outline: true, headings: 3 });
    t.tap(J);
    t.tap({ key: 'ArrowUp', code: 'ArrowUp' });
    t.tap(ENTER);
    const space = t.tap({ key: ' ', code: 'Space' });
    t.tap({ key: 'G', code: 'KeyG', shiftKey: true });
    t.tap(G);
    t.tap({ key: 'o', code: 'KeyO' });
    expect(t.log).toEqual(['outline.move:1', 'outline.move:-1', 'outline.choose', 'outline.close']);
    expect(space.defaultPrevented).toBe(true);
  });

  it('closes the outline and clears the highlights on Esc', () => {
    const open = setup({ outline: true });
    open.tap(ESC);
    expect(open.log).toEqual(['outline.close', 'find.clear']);

    const reading = setup();
    reading.tap(ESC);
    expect(reading.log).toEqual(['find.clear']);
  });

  it('stops the scroll before it opens the search with /', () => {
    for (const outline of [false, true]) {
      const t = setup({ outline });
      t.tap(SLASH);
      expect(t.log, String(outline)).toEqual(['cancelAll', 'find.open']);
    }
  });

  it('sends q, r, Backspace and Ctrl-o to the controller, with or without the outline', () => {
    for (const outline of [false, true]) {
      const t = setup({ outline });
      t.tap(Q);
      t.tap(R);
      t.tap({ key: 'Backspace', code: 'Backspace' });
      t.tap({ key: 'o', code: 'KeyO', ctrlKey: true });
      expect(t.log, String(outline)).toEqual([
        'request:quit',
        'request:reload',
        'request:back',
        'request:back',
      ]);
    }
  });

  it('swallows Enter while reading', () => {
    const t = setup();
    const down = t.tap(ENTER);
    expect(t.log).toEqual([]);
    expect(down.defaultPrevented).toBe(true);
  });

  it('matches /, [ and ] whatever Shift and Alt say', () => {
    const shifted = setup();
    shifted.tap({ key: '/', code: 'Digit7', shiftKey: true });
    expect(shifted.log).toEqual(['cancelAll', 'find.open']);

    const option = setup();
    option.tap({ key: '[', code: 'Digit5', altKey: true });
    expect(option.log).toEqual(['pending:[']);

    const altGr = setup();
    altGr.tap({ key: ']', code: 'Digit9', ctrlKey: true, altKey: true });
    expect(altGr.log).toEqual(['pending:]']);
  });

  it('leaves unbound keys, Meta chords, and Shift+Tab to the page', () => {
    const inits: KeyboardEventInit[] = [
      { key: 'x', code: 'KeyX' },
      { key: 'j', code: 'KeyJ', metaKey: true },
      { key: 'j', code: 'KeyJ', ctrlKey: true },
      { key: 'j', code: 'KeyJ', altKey: true },
      { key: 'Tab', code: 'Tab', shiftKey: true },
    ];
    for (const init of inits) {
      const t = setup();
      const down = t.tap(init);
      expect(t.log, JSON.stringify(init)).toEqual([]);
      expect(down.defaultPrevented, JSON.stringify(init)).toBe(false);
    }
  });

  it('leaves keys to a focused text field, including q, r and /', () => {
    const t = setup();
    focusInput();
    for (const init of [Q, R, SLASH, J]) {
      const down = t.tap(init);
      expect(down.defaultPrevented, init.key).toBe(false);
    }
    expect(t.log).toEqual([]);
  });

  it('handles keys while a read-only field has focus', () => {
    const t = setup();
    focusInput(true);
    t.tap(J);
    expect(t.log).toEqual(['scrollBy:60:1:press']);
  });

  it('ignores keys while an IME composes', () => {
    const t = setup();
    const down = t.tap({ ...J, isComposing: true });
    expect(t.log).toEqual([]);
    expect(down.defaultPrevented).toBe(false);
  });

  it('keeps a handled key and its keyup from later listeners', () => {
    const t = setup();
    const reached: string[] = [];
    t.win.addEventListener('keydown', () => reached.push('keydown'));
    t.win.addEventListener('keyup', () => reached.push('keyup'));
    t.tap(J);
    expect(reached).toEqual([]);
    t.tap({ key: 'x', code: 'KeyX' });
    expect(reached).toEqual(['keydown', 'keyup']);
  });

  it('forgets held keys and the pending chord when the window loses focus', () => {
    const chord = setup();
    chord.tap(G);
    chord.win.dispatchEvent(new Event('blur'));
    expect(chord.log).toEqual(['pending:g', 'pending:null']);

    const held = setup();
    held.send('keydown', J);
    held.win.dispatchEvent(new Event('blur'));
    held.send('keydown', J);
    expect(held.log).toEqual(['scrollBy:60:1:press', 'scrollBy:60:1:press']);
  });

  it('hides the toast on every key, handled or not', () => {
    const t = setup();
    t.tap(J);
    t.tap({ key: 'x', code: 'KeyX' });
    t.relay({ key: 'j' });
    expect(t.dismissed()).toBe(3);
  });

  it('runs a relayed key as a single tap', () => {
    const t = setup();
    t.relay({ key: 'j' });
    t.relay({ key: 'G', shift: true });
    t.relay({ key: 'd', ctrl: true });
    t.relay({ key: 'q' });
    expect(t.log).toEqual([
      'scrollBy:60:1:tap',
      'scrollTo:bottom:tap',
      'scrollBy:viewSize:0.5:tap',
      'request:quit',
    ]);
  });

  it('resolves a chord from relayed keys', () => {
    const t = setup();
    t.relay({ key: 'g' });
    t.relay({ key: 'g' });
    expect(t.log).toEqual(['pending:g', 'pending:null', 'scrollTo:top:tap']);
  });

  it('types relayed keys into a query being typed', () => {
    const t = setup({ stage: 'typing' });
    t.relay({ key: 'q' });
    t.relay({ key: '/' });
    t.relay({ key: 'Backspace' });
    t.relay({ key: 'Enter' });
    t.relay({ key: 'Escape' });
    t.relay({ key: 'u', ctrl: true });
    expect(t.log).toEqual([
      'find.type:q',
      'find.type:/',
      'find.backspace',
      'find.enter',
      'find.escape',
    ]);
  });

  it('types a relayed character outside the Basic Multilingual Plane into the query', () => {
    const t = setup({ stage: 'typing' });
    t.relay({ key: '😀' });
    t.relay({ key: '𠮷' });
    expect(t.log).toEqual(['find.type:😀', 'find.type:𠮷']);
  });

  it('keeps relayed keys that start a search in order', () => {
    const t = setup();
    t.relay({ key: '/' });
    t.relay({ key: 'f' });
    t.relay({ key: 'o' });
    expect(t.log).toEqual(['cancelAll', 'find.open', 'find.type:f', 'find.type:o']);
  });
});
