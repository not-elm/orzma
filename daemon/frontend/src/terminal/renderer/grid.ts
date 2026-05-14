import type { Color, Cursor, FrameDelta, FrameSnapshot, RenderFrame, Run } from '../protocol/frame';
import { widthOfGrapheme } from './font';

/** One terminal cell. */
export interface Cell {
  text: string;
  width: 0 | 1 | 2;
  fg: Color;
  bg: Color;
  style: number;
  hyperlinkId?: number;
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
}

/** Constructs an empty grid with default state. */
export function createGrid(init: { cols: number; rows: number }): Grid {
  return {
    cols: init.cols,
    rows: init.rows,
    cells: Array.from({ length: init.rows }, () => []),
    cursor: { x: 0, y: 0, shape: 'block', blinking: false, visible: true },
    modes: new Set(),
    title: '',
    dirtyRows: new Set(),
  };
}

/** Applies a snapshot or delta to the grid, marking dirty rows. */
export function applyFrame(grid: Grid, frame: RenderFrame): void {
  if (frame.kind === 'delta') {
    applyDelta(grid, frame);
  } else {
    applySnapshot(grid, frame);
  }
}

function applySnapshot(grid: Grid, frame: FrameSnapshot): void {
  grid.cols = frame.cols;
  grid.rows = frame.rows;
  grid.cells = new Array(frame.rows);
  for (let row = 0; row < frame.rows; row++) {
    grid.cells[row] = expandRunsToRow(frame.rows_data[row]?.runs ?? [], frame.cols);
    grid.dirtyRows.add(row);
  }
  grid.cursor = frame.cursor;
  grid.modes.clear();
  for (const m of frame.modes) grid.modes.add(m);
}

function applyDelta(grid: Grid, frame: FrameDelta): void {
  for (const { row, runs } of frame.dirty_rows) {
    grid.cells[row] = expandRunsToRow(runs, grid.cols);
    grid.dirtyRows.add(row);
  }
  grid.cursor = frame.cursor;
}

/** Reverses run coalescing: returns one Cell per grapheme cluster. */
export function expandRunsToRow(runs: readonly Run[], _cols: number): Cell[] {
  const cells: Cell[] = [];
  const segmenter = new Intl.Segmenter('en', { granularity: 'grapheme' });
  for (const run of runs) {
    for (const { segment } of segmenter.segment(run.text)) {
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
