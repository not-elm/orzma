import type { Color, Cursor, FrameDelta, FrameSnapshot, RenderFrame, Run } from '../protocol/frame';
import { widthOfGrapheme } from './font';

/**
 * Module-scoped grapheme segmenter. Constructing `Intl.Segmenter` is expensive
 * (browser allocates per-locale ICU data); hoisting to module scope amortizes
 * the construction across every `expandRunsToRow` call for the lifetime of the
 * page. Safe to share — `Intl.Segmenter.segment()` returns a fresh iterator.
 */
const GRAPHEME_SEGMENTER = new Intl.Segmenter('en', { granularity: 'grapheme' });

/** One terminal cell. */
export interface Cell {
  text: string;
  width: 0 | 1 | 2;
  fg: Color;
  bg: Color;
  style: number;
  hyperlinkId?: number;
}

/** Result of locating a cell at a terminal column. */
export interface CellSpan {
  cell: Cell;
  /** Starting column (inclusive). */
  startCol: number;
  /** Ending column (exclusive). For wide-char cells, `endCol - startCol === 2`. */
  endCol: number;
}

/** Renderer-side mirror of the server's terminal grid. */
export interface Grid {
  cols: number;
  rows: number;
  cells: Cell[][];
  cursor: Cursor;
  modes: Set<string>;
  title: string;
  dirtyRows: Set<number>;
  // NOTE: bumped by applyFrame independently of dirtyRows. dirtyRows is
  // consumed (cleared) by the renderer each paint; rowVersions persists so
  // pointer-overlay row-hover caches can invalidate without racing the
  // renderer.
  rowVersions: Uint32Array;
  /** Returns the cell at terminal column `col`, accounting for wide-char width. */
  cellAtColumn(row: number, col: number): CellSpan | undefined;
}

/** Returns a shallow-cloned `Grid` safe to publish to subscribers (e.g.,
 *  `gridStore.setGrid`). The `cells` array and `modes` Set are duplicated
 *  so subsequent in-place mutations from `applyFrame` cannot race with
 *  consumers that read the published reference across microtask boundaries. */
export function snapshotGrid(grid: Grid): Grid {
  return {
    ...grid,
    cells: grid.cells.slice(),
    modes: new Set(grid.modes),
  };
}

/** Constructs an empty grid with default state. */
export function createGrid(init: { cols: number; rows: number }): Grid {
  const grid: Grid = {
    cols: init.cols,
    rows: init.rows,
    cells: Array.from({ length: init.rows }, () => []),
    cursor: { x: 0, y: 0, shape: 'block', blinking: false, visible: true },
    modes: new Set(),
    title: '',
    dirtyRows: new Set(),
    rowVersions: new Uint32Array(init.rows),
    cellAtColumn(row, col) {
      return cellAtColumnImpl(this, row, col);
    },
  };
  return grid;
}

function cellAtColumnImpl(grid: Grid, row: number, col: number): CellSpan | undefined {
  const cells = grid.cells[row];
  if (!cells) return undefined;
  let runningCol = 0;
  for (const cell of cells) {
    if (cell.width === 0) continue;
    if (runningCol + cell.width > col) {
      return { cell, startCol: runningCol, endCol: runningCol + cell.width };
    }
    runningCol += cell.width;
  }
  return undefined;
}

/** Applies a snapshot or delta to the grid, marking dirty rows. */
export function applyFrame(grid: Grid, frame: RenderFrame): void {
  if (frame.kind === 'delta') {
    applyDelta(grid, frame);
  } else {
    applySnapshot(grid, frame);
  }
}

/**
 * per-row content diffing. A vim page-down or terminal scroll arrives
 * as a snapshot (Alacritty marks scroll as full damage; ozmux's bridge
 * also promotes deltas with ≥70% dirty rows to snapshots). The naive
 * "reset cells + bump every row" approach would force React.memo on every
 * <Row> to re-render every frame. By comparing the new content against the
 * previous row and preserving the cells reference + rowVersions entry when
 * nothing changed, unchanged rows skip render entirely.
 */
function applySnapshot(grid: Grid, frame: FrameSnapshot): void {
  const prevCells = grid.cells;
  const prevVersions = grid.rowVersions;
  const nextCells: Cell[][] = new Array(frame.rows);
  const nextVersions = new Uint32Array(frame.rows);
  grid.cols = frame.cols;
  grid.rows = frame.rows;
  for (let row = 0; row < frame.rows; row++) {
    const next = expandRunsToRow(frame.rows_data[row]?.runs ?? [], frame.cols);
    const prev = prevCells[row];
    const prevVersion = row < prevVersions.length ? prevVersions[row] : 0;
    if (prev !== undefined && rowsEqual(prev, next)) {
      nextCells[row] = prev;
      nextVersions[row] = prevVersion;
    } else {
      nextCells[row] = next;
      nextVersions[row] = prevVersion + 1;
      grid.dirtyRows.add(row);
    }
  }
  grid.cells = nextCells;
  grid.rowVersions = nextVersions;
  grid.cursor = frame.cursor;
  grid.modes.clear();
  for (const m of frame.modes) grid.modes.add(m);
}

function colorsEqual(a: Color, b: Color): boolean {
  if (a === b) return true;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a[0] === b[0] && a[1] === b[1] && a[2] === b[2];
  }
  return false;
}

function cellsEqual(a: Cell, b: Cell): boolean {
  return (
    a.text === b.text &&
    a.width === b.width &&
    a.style === b.style &&
    a.hyperlinkId === b.hyperlinkId &&
    colorsEqual(a.fg, b.fg) &&
    colorsEqual(a.bg, b.bg)
  );
}

function rowsEqual(a: readonly Cell[], b: readonly Cell[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const ai = a[i];
    const bi = b[i];
    if (ai === undefined || bi === undefined) return ai === bi;
    if (!cellsEqual(ai, bi)) return false;
  }
  return true;
}

function applyDelta(grid: Grid, frame: FrameDelta): void {
  if (frame.dirty_rows.length > 0) {
    // NOTE: clone the rowVersions buffer so the reference changes. grid-store's
    // shallow equality compares rowVersions by reference (===); mutating the
    // typed array in place would let delta updates pass through unnoticed
    // whenever cursor + geometry stayed the same.
    const nextVersions = new Uint32Array(grid.rowVersions);
    for (const { row, runs } of frame.dirty_rows) {
      grid.cells[row] = expandRunsToRow(runs, grid.cols);
      grid.dirtyRows.add(row);
      if (row < nextVersions.length) {
        nextVersions[row] += 1;
      }
    }
    grid.rowVersions = nextVersions;
  }
  grid.cursor = frame.cursor;
}

/** Reverses run coalescing: returns one Cell per grapheme cluster. */
export function expandRunsToRow(runs: readonly Run[], _cols: number): Cell[] {
  const cells: Cell[] = [];
  for (const run of runs) {
    for (const { segment } of GRAPHEME_SEGMENTER.segment(run.text)) {
      const w = widthOfGrapheme(segment);
      cells.push({
        text: segment,
        width: w,
        fg: run.fg,
        bg: run.bg,
        style: run.style,
        hyperlinkId: run.hyperlink_id ?? undefined,
      });
    }
  }
  return cells;
}
