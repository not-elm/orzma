/*!
 * The scrolling below is ported from Vimium's content_scripts/scroller.js
 * (https://github.com/philc/vimium, commit e34b529328).
 *
 * Copyright (c) 2010 Phil Crosby, Ilya Sukhar.
 * Vimium is released under the MIT License; see licenses/vimium/MIT-LICENSE.txt.
 */

import type { HeldKeys, Press } from './held';

/** The time source and frame scheduler a scroller runs on. */
export interface Clock {
  /** The current time, on the timebase of `KeyboardEvent.timeStamp`. */
  now(): number;
  /** Schedules `callback` for the next frame and returns a handle for `cancelFrame`. */
  requestFrame(callback: (timestamp: number) => void): number;
  /** Cancels a frame scheduled with `requestFrame`. */
  cancelFrame(handle: number): void;
}

/** A scroll the TUI or a key asks for, named as on the wire. */
export type ScrollAction =
  | 'down'
  | 'up'
  | 'halfDown'
  | 'halfUp'
  | 'pageDown'
  | 'pageUp'
  | 'top'
  | 'bottom';

/** Scrolls the page the way Vimium does. */
export interface Scroller {
  /**
   * Scrolls by `amount × factor` pixels, where `'viewSize'` stands for the viewport height. The
   * scroll keeps going while `press` is held; without `press` it is a single tap, and a repeat
   * press starts nothing.
   */
  scrollBy(amount: number | 'viewSize', factor: number, press?: Press): void;
  /** Scrolls to the top or the bottom, continuing while `press` is held. */
  scrollTo(position: 'top' | 'bottom', press?: Press): void;
  /** Stops every running scroll at once. */
  cancelAll(): void;
}

/** The distance of a line scroll, in CSS pixels. */
export const LINE = 60;

const MIN_CALIBRATION = 0.5;
const MAX_CALIBRATION = 1.6;
const CALIBRATION_BOUNDARY = 150;
const CALIBRATION_DELAY_MS = 75;
const FRAME_MS = 1000 / 60;
const ELEMENT_NODE = 1;
const DOCUMENT_FRAGMENT_NODE = 11;

/** A `Clock` on `win`'s `performance.now` and animation frames. */
export function windowClock(win: Window): Clock {
  return {
    now: () => win.performance.now(),
    requestFrame: (callback) => win.requestAnimationFrame(callback),
    cancelFrame: (handle) => win.cancelAnimationFrame(handle),
  };
}

/** Runs the scroll `action` names; without `press` it is a single tap. */
export function runScrollAction(scroller: Scroller, action: ScrollAction, press?: Press): void {
  switch (action) {
    case 'down':
      scroller.scrollBy(LINE, 1, press);
      break;
    case 'up':
      scroller.scrollBy(LINE, -1, press);
      break;
    case 'halfDown':
      scroller.scrollBy('viewSize', 0.5, press);
      break;
    case 'halfUp':
      scroller.scrollBy('viewSize', -0.5, press);
      break;
    case 'pageDown':
      scroller.scrollBy('viewSize', 1, press);
      break;
    case 'pageUp':
      scroller.scrollBy('viewSize', -1, press);
      break;
    case 'top':
      scroller.scrollTo('top', press);
      break;
    case 'bottom':
      scroller.scrollTo('bottom', press);
      break;
  }
}

/**
 * Installs a scroller on `win`. It scrolls the element the user last clicked, or its nearest
 * scrollable ancestor, and otherwise the document or its largest visible scrollable element.
 */
export function installScroller(win: Window, clock: Clock, held: HeldKeys): Scroller {
  const doc = win.document;
  const running = new Set<{ handle: number }>();
  let activated: Element | null = null;

  win.addEventListener(
    'click',
    (event) => {
      const target = event.composedPath()[0];
      if (isElement(target)) {
        activated = target;
      }
    },
    true,
  );

  const scrollingElement = (): Element | null => doc.scrollingElement ?? doc.body;

  function performScroll(el: Element, amount: number): boolean {
    const before = el.scrollTop;
    el.scrollBy({ top: amount, behavior: 'instant' });
    return el.scrollTop !== before;
  }

  function dimension(el: Element, amount: number | 'viewSize'): number {
    if (amount !== 'viewSize') {
      return amount;
    }
    return el === scrollingElement() ? win.innerHeight : el.clientHeight;
  }

  function shouldScroll(el: Element): boolean {
    const style = win.getComputedStyle(el);
    if (style.getPropertyValue('overflow-y') === 'hidden') {
      return false;
    }
    if (['hidden', 'collapse'].includes(style.getPropertyValue('visibility'))) {
      return false;
    }
    return style.getPropertyValue('display') !== 'none';
  }

  function doesScroll(el: Element, direction: number): boolean {
    const delta = Math.sign(direction) || -1;
    return performScroll(el, delta) && performScroll(el, -delta);
  }

  function containing(el: Element): Element | null {
    if (el.assignedSlot) {
      return el.assignedSlot;
    }
    if (el.parentElement !== null) {
      return el.parentElement;
    }
    const root = el.getRootNode();
    return root.nodeType === DOCUMENT_FRAGMENT_NODE && 'host' in root
      ? (root as ShadowRoot).host
      : null;
  }

  function findScrollable(start: Element, direction: number): Element {
    const top = scrollingElement();
    let el = start;
    while (el !== top && !(doesScroll(el, direction) && shouldScroll(el))) {
      const parent = containing(el) ?? top;
      if (parent === null) {
        break;
      }
      el = parent;
    }
    return el;
  }

  function visibleArea(el: Element): number {
    if (!el.checkVisibility({ visibilityProperty: true, opacityProperty: true })) {
      return 0;
    }
    const box = el.getBoundingClientRect();
    const width = Math.min(box.right, win.innerWidth) - Math.max(box.left, 0);
    const height = Math.min(box.bottom, win.innerHeight) - Math.max(box.top, 0);
    return width > 0 && height > 0 ? width * height : 0;
  }

  function firstScrollable(from: Element | null = null): Element | null {
    let el = from;
    if (el === null) {
      const top = scrollingElement();
      if (top === null) {
        return null;
      }
      if (doesScroll(top, 1) || doesScroll(top, -1)) {
        return top;
      }
      el = doc.body ?? top;
    }
    if (doesScroll(el, 1) || doesScroll(el, -1)) {
      return el;
    }
    const children = Array.from(el.children)
      .map((child) => ({ child, area: visibleArea(child) }))
      .filter(({ area }) => area > 0)
      .sort((a, b) => b.area - a.area);
    for (const { child } of children) {
      const found = firstScrollable(child);
      if (found !== null) {
        return found;
      }
    }
    return null;
  }

  function target(): Element | null {
    if (activated !== null && !activated.isConnected) {
      activated = null;
    }
    if (activated === null) {
      activated = firstScrollable() ?? scrollingElement();
    }
    return activated;
  }

  function followIfOffscreen(el: Element): void {
    if (activated === null) {
      return;
    }
    const box = activated.getBoundingClientRect();
    if (box.bottom < 0 || box.top > win.innerHeight || box.right < 0 || box.left > win.innerWidth) {
      activated = el;
    }
  }

  function animate(el: Element, amount: number, press: Press | undefined): void {
    if (amount === 0) {
      return;
    }
    const sign = Math.sign(amount);
    const total = Math.abs(amount);
    const duration = Math.max(100, 20 * Math.log(total));
    const stillDown = (): boolean => press !== undefined && held.isHeld(press);
    const animation = { handle: 0 };
    let moved = 0;
    let elapsedTotal = 0;
    let calibration = 1;
    let previous = press?.timeStamp ?? clock.now();

    const frame = (timestamp: number): void => {
      const elapsed = Math.max(0, timestamp - previous);
      previous = timestamp;
      if (elapsed === 0) {
        animation.handle = clock.requestFrame(frame);
        return;
      }
      elapsedTotal += elapsed;
      if (stillDown() && elapsedTotal >= CALIBRATION_DELAY_MS) {
        const steps = elapsed / FRAME_MS;
        if (1.05 * calibration * total < CALIBRATION_BOUNDARY) {
          calibration *= 1.05 ** steps;
        }
        if (CALIBRATION_BOUNDARY < 0.95 * calibration * total) {
          calibration *= 0.95 ** steps;
        }
        calibration = Math.min(MAX_CALIBRATION, Math.max(MIN_CALIBRATION, calibration));
      }
      let delta = Math.ceil(total * (elapsed / duration) * calibration);
      if (!stillDown()) {
        delta = Math.max(0, Math.min(delta, total - moved));
      }
      if (delta > 0 && performScroll(el, sign * delta)) {
        moved += delta;
        animation.handle = clock.requestFrame(frame);
        return;
      }
      running.delete(animation);
      followIfOffscreen(el);
    };

    running.add(animation);
    animation.handle = clock.requestFrame(frame);
  }

  return {
    scrollBy(amount, factor, press) {
      if (press?.repeat) {
        return;
      }
      const start = target();
      if (start === null) {
        return;
      }
      const el = findScrollable(start, factor);
      animate(el, factor * dimension(el, amount), press);
    },
    scrollTo(position, press) {
      if (press?.repeat) {
        return;
      }
      const start = target();
      if (start === null) {
        return;
      }
      const el = findScrollable(start, position === 'top' ? -1 : 1);
      animate(el, (position === 'top' ? 0 : el.scrollHeight) - el.scrollTop, press);
    },
    cancelAll() {
      for (const animation of running) {
        clock.cancelFrame(animation.handle);
      }
      running.clear();
    },
  };
}

function isElement(target: EventTarget | undefined): target is Element {
  return target !== undefined && (target as Node).nodeType === ELEMENT_NODE;
}
