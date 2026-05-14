//! External store for overlay-relevant grid state — driven by `useCanvasTerminal`'s
//! frame handler, consumed by overlays via `useSyncExternalStore`. Bypasses
//! React state so cursor moves don't re-run the hook every frame.

import { useSyncExternalStore } from 'react';
import type { Cursor } from './protocol/frame';
import type { FontMetrics } from './renderer/font';

/** Overlay state read by Cursor / IME / Selection / Link components. */
export interface OverlayState {
  cursor: Cursor;
  cols: number;
  rows: number;
  fm: FontMetrics;
}

const INITIAL_CURSOR: Cursor = {
  x: 0,
  y: 0,
  shape: 'block',
  blinking: false,
  visible: true,
};

const INITIAL_FM: FontMetrics = {
  cellW: 8,
  cellH: 16,
  baseline: 12,
  fontCss: '14px monospace',
  dpr: 1,
};

let state: OverlayState = {
  cursor: INITIAL_CURSOR,
  cols: 80,
  rows: 24,
  fm: INITIAL_FM,
};

const listeners = new Set<() => void>();

function shallowEqualCursor(a: Cursor, b: Cursor): boolean {
  return (
    a.x === b.x &&
    a.y === b.y &&
    a.shape === b.shape &&
    a.blinking === b.blinking &&
    a.visible === b.visible
  );
}

function shallowEqualState(a: OverlayState, b: OverlayState): boolean {
  return (
    a.cols === b.cols &&
    a.rows === b.rows &&
    // NOTE: reference equality is intentional — fm is reference-stable inside an effect run.
    a.fm === b.fm &&
    shallowEqualCursor(a.cursor, b.cursor)
  );
}

/** Updates the overlay store and notifies subscribers if the state actually changed. */
export function setOverlayState(next: OverlayState): void {
  if (shallowEqualState(state, next)) return;
  state = next;
  for (const l of listeners) l();
}

/** Subscribes a React component to the overlay store. */
export function useOverlayState(): OverlayState {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => state,
    () => state,
  );
}
