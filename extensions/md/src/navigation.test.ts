import { describe, expect, it } from 'vitest';
import { COL_STEP, handleKey, type KeyState, LINE_STEP, type ScrollMetrics } from './navigation.ts';

const m: ScrollMetrics = { scrollTop: 100, scrollLeft: 50, clientHeight: 200 };
const fresh = (): KeyState => ({ lastGAt: 0 });
const k = (key: string, ctrlKey = false) => ({ key, ctrlKey });

describe('handleKey', () => {
  it('j scrolls down one line, k scrolls up', () => {
    expect(handleKey(k('j'), m, fresh(), 1000, 1000)).toEqual({ top: 100 + LINE_STEP });
    expect(handleKey(k('k'), m, fresh(), 1000, 1000)).toEqual({ top: 100 - LINE_STEP });
  });

  it('h/l scroll left/right', () => {
    expect(handleKey(k('l'), m, fresh(), 1000, 1000)).toEqual({ left: 50 + COL_STEP });
    expect(handleKey(k('h'), m, fresh(), 1000, 1000)).toEqual({ left: 50 - COL_STEP });
  });

  it('Ctrl-d / Ctrl-u scroll half a page', () => {
    expect(handleKey(k('d', true), m, fresh(), 1000, 1000)).toEqual({ top: 100 + 100 });
    expect(handleKey(k('u', true), m, fresh(), 1000, 1000)).toEqual({ top: 100 - 100 });
  });

  it('G jumps to the bottom (maxTop)', () => {
    expect(handleKey(k('G'), m, fresh(), 1000, 777)).toEqual({ top: 777 });
  });

  it('clamps to [0, maxTop]', () => {
    const atTop: ScrollMetrics = { scrollTop: 10, scrollLeft: 0, clientHeight: 200 };
    expect(handleKey(k('k'), atTop, fresh(), 1000, 1000)).toEqual({ top: 0 });
    const atBottom: ScrollMetrics = { scrollTop: 990, scrollLeft: 0, clientHeight: 200 };
    expect(handleKey(k('j'), atBottom, fresh(), 1000, 1000)).toEqual({ top: 1000 });
  });

  it('gg (two g within the window) jumps to top; a single g does nothing', () => {
    const st = fresh();
    expect(handleKey(k('g'), m, st, 1000, 1000)).toBeNull();
    expect(handleKey(k('g'), m, st, 1100, 1000)).toEqual({ top: 0 });
  });

  it('g then a non-g resets the gg sequence', () => {
    const st = fresh();
    expect(handleKey(k('g'), m, st, 1000, 1000)).toBeNull();
    handleKey(k('x'), m, st, 1050, 1000);
    expect(handleKey(k('g'), m, st, 1100, 1000)).toBeNull();
  });

  it('returns null for unhandled keys', () => {
    expect(handleKey(k('z'), m, fresh(), 1000, 1000)).toBeNull();
  });
});
