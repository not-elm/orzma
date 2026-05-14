//! External store for the Grid model — driven by `useCanvasTerminal`'s frame
//! handler, consumed by <TerminalGrid> via `useSyncExternalStore`. Bypasses
//! React hook state so frame updates don't re-run the hook every frame
//! (preserves Phase 3B invariant: hook re-renders only on preedit / isActive).

import { useSyncExternalStore } from 'react';
import type { Cursor } from '../protocol/frame';
import { createGrid, type Grid } from './grid';

let state: Grid = createGrid({ cols: 80, rows: 24 });
const listeners = new Set<() => void>();

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

/** Updates the grid store and notifies subscribers iff the state actually
 *  changed at the level of rowVersions reference, cursor fields, or geometry. */
export function setGrid(next: Grid): void {
  if (gridShallowEqual(state, next)) return;
  state = next;
  for (const l of listeners) l();
}

/** Subscribes a React component to the grid store. */
export function useGridStore(): Grid {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => state,
    () => state,
  );
}
