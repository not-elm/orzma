//! Per-Terminal external store for the Grid model — created via factory in
//! useCanvasTerminal, provided via Context to descendants. Each <Terminal>
//! instance gets its own store so split panes don't share cursor / cells.
//! Bypasses React hook state so frame updates don't re-run the hook every
//! frame (preserves Phase 3B invariant: hook re-renders only on preedit /
//! isActive).

import { createContext, useContext, useSyncExternalStore } from 'react';
import type { Cursor } from '../protocol/frame';
import { createGrid, type Grid } from './grid';

export interface GridStore {
  setGrid(next: Grid): void;
  subscribe(cb: () => void): () => void;
  getSnapshot(): Grid;
}

function cursorEqual(a: Cursor, b: Cursor): boolean {
  return (
    a.x === b.x &&
    a.y === b.y &&
    a.shape === b.shape &&
    a.blinking === b.blinking &&
    a.visible === b.visible
  );
}

function gridShallowEqual(a: Grid, b: Grid): boolean {
  return (
    a.cols === b.cols &&
    a.rows === b.rows &&
    a.rowVersions === b.rowVersions &&
    cursorEqual(a.cursor, b.cursor)
  );
}

/** Creates a per-Terminal grid store. Use in useCanvasTerminal via useRef
 *  so each hook instance owns its own state. */
export function createGridStore(): GridStore {
  let state: Grid = createGrid({ cols: 80, rows: 24 });
  const listeners = new Set<() => void>();
  return {
    setGrid(next: Grid): void {
      if (gridShallowEqual(state, next)) return;
      state = next;
      for (const l of listeners) l();
    },
    subscribe(cb: () => void): () => void {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    getSnapshot(): Grid {
      return state;
    },
  };
}

export const GridStoreContext = createContext<GridStore | null>(null);

/** Reads the grid from the nearest GridStoreContext.Provider. */
export function useGridStore(): Grid {
  const store = useContext(GridStoreContext);
  if (!store) {
    throw new Error('useGridStore must be used inside a GridStoreContext.Provider');
  }
  return useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
}
