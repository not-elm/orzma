import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { HeldKeys } from './held';
import {
  __testables,
  type Clock,
  installScroller,
  runScrollAction,
  type ScrollerOptions,
} from './scroller';

const { LINE } = __testables;

interface Frames {
  clock: Clock;
  step(ms: number): void;
  runFor(ms: number, frameMs?: number): void;
  scheduled(): number;
}

function frames(): Frames {
  let now = 1000;
  let queue: { id: number; callback: (timestamp: number) => void }[] = [];
  let nextId = 1;
  const step = (ms: number): void => {
    now += ms;
    const due = queue;
    queue = [];
    for (const frame of due) {
      frame.callback(now);
    }
  };
  return {
    clock: {
      now: () => now,
      requestFrame: (callback) => {
        const id = nextId++;
        queue.push({ id, callback });
        return id;
      },
      cancelFrame: (id) => {
        queue = queue.filter((frame) => frame.id !== id);
      },
    },
    step,
    runFor: (ms, frameMs = 1000 / 60) => {
      for (let elapsed = 0; elapsed < ms; elapsed += frameMs) {
        step(frameMs);
      }
    },
    scheduled: () => queue.length,
  };
}

function rect(width: number, height: number): DOMRect {
  return {
    x: 0,
    y: 0,
    left: 0,
    top: 0,
    right: width,
    bottom: height,
    width,
    height,
    toJSON: () => ({}),
  } as DOMRect;
}

function scrollable(height = 10_000, view = 600, box = rect(800, view)): HTMLElement {
  const el = document.createElement('div');
  let top = 0;
  Object.defineProperties(el, {
    scrollTop: {
      configurable: true,
      get: () => top,
      set: (value: number) => {
        top = value;
      },
    },
    scrollHeight: { configurable: true, value: height },
    clientHeight: { configurable: true, value: view },
    scrollBy: {
      configurable: true,
      value: (options: ScrollToOptions) => {
        top = Math.min(Math.max(top + (options.top ?? 0), 0), height - view);
      },
    },
    getBoundingClientRect: { configurable: true, value: () => box },
  });
  document.body.append(el);
  return el;
}

function setup<T extends Element | null>(page: T, options?: ScrollerOptions) {
  const f = frames();
  const held = new HeldKeys();
  Object.defineProperty(document, 'scrollingElement', { configurable: true, get: () => page });
  const scroller = installScroller(window, f.clock, held, options);
  return { f, held, page, scroller };
}

beforeEach(() => {
  document.body.replaceChildren();
  Object.defineProperty(Element.prototype, 'checkVisibility', {
    configurable: true,
    value: () => true,
  });
  Object.defineProperty(Element.prototype, 'scrollBy', { configurable: true, value: () => {} });
});

afterEach(() => {
  if (document.body === null) {
    document.documentElement.append(document.createElement('body'));
  }
});

describe('installScroller', () => {
  it('moves a tap exactly one line', () => {
    const { f, held, page, scroller } = setup(scrollable());
    scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()));
    f.step(16);
    held.release('KeyJ');
    f.runFor(150);
    expect(page.scrollTop).toBe(60);
    expect(f.scheduled()).toBe(0);
  });

  it('moves from the first frame', () => {
    const { f, held, page, scroller } = setup(scrollable());
    scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()));
    f.step(16);
    expect(page.scrollTop).toBeGreaterThan(0);
  });

  it('adds up two quick taps', () => {
    const { f, held, page, scroller } = setup(scrollable());
    for (let i = 0; i < 2; i++) {
      scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()));
      f.step(16);
      held.release('KeyJ');
    }
    f.runFor(300);
    expect(page.scrollTop).toBe(120);
  });

  it('keeps scrolling while the key is held and stops on release', () => {
    const { f, held, page, scroller } = setup(scrollable());
    scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()));
    for (let i = 0; i < 30; i++) {
      f.step(33);
      scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()));
      expect(f.scheduled()).toBe(1);
    }
    const whileHeld = page.scrollTop;
    expect(whileHeld).toBeGreaterThan(300);
    held.release('KeyJ');
    f.runFor(500, 33);
    expect(page.scrollTop).toBe(whileHeld);
    expect(f.scheduled()).toBe(0);
  });

  it('keeps a held scroll going while another key goes down', () => {
    const { f, held, page, scroller } = setup(scrollable());
    scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()));
    f.step(16);
    held.press('ShiftLeft', f.clock.now());
    f.runFor(500);
    expect(page.scrollTop).toBeGreaterThan(LINE);
  });

  it('keeps the held speed within the calibration bounds', () => {
    const { f, held, page, scroller } = setup(scrollable(1_000_000));
    scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()));
    f.runFor(2000);
    const before = page.scrollTop;
    f.runFor(500);
    const speed = (page.scrollTop - before) / 500;
    expect(speed).toBeGreaterThan(0.9);
    expect(speed).toBeLessThan((LINE / 100) * 1.6 * 1.1);
  });

  it('ramps up at the same pace at 30 and 60 frames a second', () => {
    const rampMs = (frameMs: number): number => {
      const { f, held, page, scroller } = setup(scrollable(1_000_000));
      scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()));
      const samples = [{ t: 0, top: 0 }];
      for (let t = frameMs; t <= 1500; t += frameMs) {
        f.step(frameMs);
        samples.push({ t, top: page.scrollTop });
        const earlier = samples.find((sample) => sample.t >= t - 100);
        if (
          t >= 100 &&
          earlier !== undefined &&
          (page.scrollTop - earlier.top) / (t - earlier.t) >= 0.9
        ) {
          return t;
        }
      }
      return Number.POSITIVE_INFINITY;
    };
    const at60 = rampMs(1000 / 60);
    const at30 = rampMs(1000 / 30);
    expect(at60).toBeLessThan(500);
    expect(Math.abs(at60 - at30)).toBeLessThanOrEqual(50);
  });

  it('slows a held page scroll down to the lower calibration bound', () => {
    const { f, held, page, scroller } = setup(scrollable(10_000_000));
    scroller.scrollBy('viewSize', 1, held.press('PageDown', f.clock.now()));
    f.runFor(2000);
    const before = page.scrollTop;
    f.runFor(500);
    const view = window.innerHeight;
    const floor = (view / Math.max(100, 20 * Math.log(view))) * 0.5;
    const speed = (page.scrollTop - before) / 500;
    expect(speed).toBeGreaterThan(floor * 0.95);
    expect(speed).toBeLessThan(floor * 1.15);
  });

  it('stops every scroll at once on cancel', () => {
    const { f, held, page, scroller } = setup(scrollable());
    scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()));
    f.step(16);
    const moved = page.scrollTop;
    scroller.cancelAll();
    f.runFor(300);
    expect(page.scrollTop).toBe(moved);
    expect(f.scheduled()).toBe(0);
  });

  it('finishes the tap but ends the hold when the held keys are forgotten', () => {
    const { f, held, page, scroller } = setup(scrollable());
    scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()));
    f.step(16);
    held.clear();
    f.runFor(300);
    expect(page.scrollTop).toBe(60);
  });

  it('runs a TUI scroll as a tap, even while a key is held', () => {
    const { f, held, page, scroller } = setup(scrollable());
    runScrollAction(scroller, 'down');
    f.runFor(300);
    expect(page.scrollTop).toBe(60);

    scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()));
    f.step(16);
    runScrollAction(scroller, 'down');
    expect(f.scheduled()).toBe(2);
  });

  it('scrolls to the top and to the bottom', () => {
    const { f, page, scroller } = setup(scrollable());
    page.scrollTop = 5000;
    runScrollAction(scroller, 'top');
    f.runFor(1000);
    expect(page.scrollTop).toBe(0);
    runScrollAction(scroller, 'bottom');
    f.runFor(1000);
    expect(page.scrollTop).toBe(10_000 - 600);
  });

  it('scrolls the largest visible scrollable element when the document does not scroll', () => {
    const small = scrollable(10_000, 100, rect(100, 100));
    const big = scrollable(10_000, 600, rect(800, 600));
    const { f, scroller } = setup(document.createElement('div'));
    runScrollAction(scroller, 'down');
    f.runFor(300);
    expect(big.scrollTop).toBe(60);
    expect(small.scrollTop).toBe(0);
  });

  it('scrolls the clicked element, and looks again once it leaves the page', () => {
    const { f, page, scroller } = setup(scrollable());
    const inner = scrollable(5000, 300, rect(400, 300));
    inner.dispatchEvent(new MouseEvent('click', { bubbles: true, composed: true }));
    runScrollAction(scroller, 'down');
    f.runFor(300);
    expect(inner.scrollTop).toBe(60);
    inner.remove();
    runScrollAction(scroller, 'down');
    f.runFor(300);
    expect(page.scrollTop).toBe(60);
  });

  it('skips an element whose overflow hides its scrolling', () => {
    const { f, page, scroller } = setup(scrollable());
    const clipped = scrollable(5000, 300, rect(400, 300));
    clipped.style.overflowY = 'hidden';
    clipped.dispatchEvent(new MouseEvent('click', { bubbles: true, composed: true }));
    runScrollAction(scroller, 'down');
    f.runFor(300);
    expect(clipped.scrollTop).toBe(0);
    expect(page.scrollTop).toBe(60);
  });

  it('scrolls a shadow host from an element inside its shadow root', () => {
    const { f, page, scroller } = setup(scrollable());
    const host = scrollable(5000, 300, rect(400, 300));
    const inner = document.createElement('div');
    host.attachShadow({ mode: 'open' }).append(inner);
    inner.dispatchEvent(new MouseEvent('click', { bubbles: true, composed: true }));
    runScrollAction(scroller, 'down');
    f.runFor(300);
    expect(host.scrollTop).toBe(60);
    expect(page.scrollTop).toBe(0);
  });

  it('does nothing on a document with nothing to scroll', () => {
    const { f, scroller } = setup(null);
    document.body.remove();
    expect(runScrollAction(scroller, 'down')).toBe(false);
    expect(runScrollAction(scroller, 'top')).toBe(false);
    expect(f.scheduled()).toBe(0);
  });

  it('reports that it started a scroll, but not for a repeat press', () => {
    const { f, held, scroller } = setup(scrollable());
    expect(runScrollAction(scroller, 'down')).toBe(true);
    expect(scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()))).toBe(true);
    expect(scroller.scrollBy(LINE, 1, held.press('KeyJ', f.clock.now()))).toBe(false);
    expect(runScrollAction(scroller, 'bottom')).toBe(true);
  });

  it('starts nothing and reports it at the edge it would scroll past', () => {
    const { f, page, scroller } = setup(scrollable());
    expect(runScrollAction(scroller, 'up')).toBe(false);
    expect(runScrollAction(scroller, 'top')).toBe(false);
    page.scrollTop = 10_000 - 600;
    expect(runScrollAction(scroller, 'down')).toBe(false);
    expect(runScrollAction(scroller, 'bottom')).toBe(false);
    expect(f.scheduled()).toBe(0);
  });

  it('reports that it cannot scroll a document whose content fits', () => {
    const { f, scroller } = setup(scrollable(600, 600));
    expect(runScrollAction(scroller, 'down')).toBe(false);
    expect(runScrollAction(scroller, 'halfUp')).toBe(false);
    expect(f.scheduled()).toBe(0);
  });

  it('leaves the top inset out of a page scroll of the document', () => {
    const { f, page, scroller } = setup(scrollable(), { topInset: 30 });
    runScrollAction(scroller, 'pageDown');
    f.runFor(1000);
    expect(page.scrollTop).toBe(window.innerHeight - 30);
    runScrollAction(scroller, 'halfUp');
    f.runFor(1000);
    expect(page.scrollTop).toBe((window.innerHeight - 30) / 2);
  });

  it('pages an element by its own height even with a top inset', () => {
    const { f, scroller } = setup(scrollable(), { topInset: 30 });
    const inner = scrollable(5000, 300, rect(400, 300));
    inner.dispatchEvent(new MouseEvent('click', { bubbles: true, composed: true }));
    runScrollAction(scroller, 'pageDown');
    f.runFor(1000);
    expect(inner.scrollTop).toBe(300);
  });
});
