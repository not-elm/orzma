//! Per-Terminal external store for overlay-relevant grid state (cursor,
//! cols/rows, font metrics). Created via factory in useCanvasTerminal,
//! provided via Context so Cursor / IME read the pane's own state.

import { createContext, useContext, useSyncExternalStore } from 'react';
import type { Cursor } from './protocol/frame';
import type { FontMetrics } from './renderer/font';

/** Overlay state read by Cursor / IME components. */
export interface OverlayState {
  cursor: Cursor;
  cols: number;
  rows: number;
  fm: FontMetrics;
}

export interface OverlayStore {
  setOverlayState(next: OverlayState): void;
  subscribe(cb: () => void): () => void;
  getSnapshot(): OverlayState;
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

/** Creates a per-Terminal overlay store. */
export function createOverlayStore(): OverlayStore {
  let state: OverlayState = {
    cursor: INITIAL_CURSOR,
    cols: 80,
    rows: 24,
    fm: INITIAL_FM,
  };
  const listeners = new Set<() => void>();
  return {
    setOverlayState(next: OverlayState): void {
      if (shallowEqualState(state, next)) return;
      state = next;
      for (const l of listeners) l();
    },
    subscribe(cb: () => void): () => void {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    getSnapshot(): OverlayState {
      return state;
    },
  };
}

export const OverlayStoreContext = createContext<OverlayStore | null>(null);

/** Reads overlay state from the nearest OverlayStoreContext.Provider. */
export function useOverlayState(): OverlayState {
  const store = useContext(OverlayStoreContext);
  if (!store) {
    throw new Error('useOverlayState must be used inside an OverlayStoreContext.Provider');
  }
  return useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
}
