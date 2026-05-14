import { describe, expect, it } from 'vitest';
import type { FrameDelta, FrameSnapshot } from '../protocol/frame';
import { applyFrame, createGrid, expandRunsToRow } from './grid';

function snapshot(over: Partial<FrameSnapshot> = {}): FrameSnapshot {
  return {
    seq: 0,
    cols: 3,
    rows: 1,
    cursor: { x: 0, y: 0, shape: 'block', visible: true },
    rows_data: [
      {
        runs: [
          {
            cols: 3,
            fg: null,
            bg: null,
            style: 0,
            text: 'abc',
            hyperlink_id: null,
          },
        ],
      },
    ],
    reason: 'initial',
    modes: [],
    ...over,
  };
}

describe('createGrid', () => {
  it('initializes empty cells, no dirty rows, cursor at 0,0', () => {
    const g = createGrid({ cols: 80, rows: 24 });
    expect(g.cols).toBe(80);
    expect(g.rows).toBe(24);
    expect(g.cells.length).toBe(24);
    expect(g.cursor).toEqual({ x: 0, y: 0, shape: 'block', visible: true });
    expect(g.dirtyRows.size).toBe(0);
    expect(g.modes.size).toBe(0);
  });
});

describe('applyFrame snapshot', () => {
  it('replaces grid and marks all rows dirty', () => {
    const g = createGrid({ cols: 80, rows: 24 });
    applyFrame(g, snapshot());
    expect(g.cols).toBe(3);
    expect(g.rows).toBe(1);
    expect(g.cells[0].length).toBe(3);
    expect(g.cells[0][0].text).toBe('a');
    expect(g.cells[0][1].text).toBe('b');
    expect(g.cells[0][2].text).toBe('c');
    expect(g.dirtyRows.has(0)).toBe(true);
  });

  it('populates modes from snapshot.modes array', () => {
    const g = createGrid({ cols: 80, rows: 24 });
    applyFrame(g, snapshot({ modes: ['alt-screen', 'bracketed-paste'] }));
    expect(g.modes.has('alt-screen')).toBe(true);
    expect(g.modes.has('bracketed-paste')).toBe(true);
  });
});

describe('applyFrame delta', () => {
  it('replaces only the specified rows and marks them dirty', () => {
    const g = createGrid({ cols: 3, rows: 3 });
    applyFrame(
      g,
      snapshot({
        cols: 3,
        rows: 3,
        rows_data: [
          {
            runs: [{ cols: 3, fg: null, bg: null, style: 0, text: 'aaa', hyperlink_id: null }],
          },
          {
            runs: [{ cols: 3, fg: null, bg: null, style: 0, text: 'bbb', hyperlink_id: null }],
          },
          {
            runs: [{ cols: 3, fg: null, bg: null, style: 0, text: 'ccc', hyperlink_id: null }],
          },
        ],
      }),
    );
    g.dirtyRows.clear();

    const delta: FrameDelta = {
      kind: 'delta',
      seq: 1,
      dirty_rows: [
        {
          row: 1,
          runs: [{ cols: 3, fg: null, bg: null, style: 0, text: 'XYZ', hyperlink_id: null }],
        },
      ],
    };
    applyFrame(g, delta);
    expect(g.cells[0][0].text).toBe('a');
    expect(g.cells[1][0].text).toBe('X');
    expect(g.cells[1][1].text).toBe('Y');
    expect(g.cells[1][2].text).toBe('Z');
    expect(g.cells[2][0].text).toBe('c');
    expect(g.dirtyRows.has(1)).toBe(true);
    expect(g.dirtyRows.has(0)).toBe(false);
    expect(g.dirtyRows.has(2)).toBe(false);
  });
});

describe('expandRunsToRow', () => {
  it('flattens single ASCII run', () => {
    const cells = expandRunsToRow(
      [{ cols: 3, fg: null, bg: null, style: 0, text: 'abc', hyperlink_id: null }],
      3,
    );
    expect(cells.length).toBe(3);
    expect(cells.map((c) => c.text).join('')).toBe('abc');
    expect(cells.every((c) => c.width === 1)).toBe(true);
  });

  it('expands wide char "あ" to a single cell with width=2', () => {
    const cells = expandRunsToRow(
      [{ cols: 2, fg: null, bg: null, style: 0, text: 'あ', hyperlink_id: null }],
      2,
    );
    expect(cells.length).toBe(1);
    expect(cells[0].text).toBe('あ');
    expect(cells[0].width).toBe(2);
  });

  it('preserves attribute boundaries across runs', () => {
    const cells = expandRunsToRow(
      [
        { cols: 1, fg: null, bg: null, style: 1, text: 'a', hyperlink_id: null }, // bold
        { cols: 1, fg: null, bg: null, style: 0, text: 'b', hyperlink_id: null },
      ],
      2,
    );
    expect(cells.length).toBe(2);
    expect(cells[0].style).toBe(1);
    expect(cells[1].style).toBe(0);
  });

  it('handles combining marks within a grapheme cluster', () => {
    const nfc = expandRunsToRow(
      [{ cols: 1, fg: null, bg: null, style: 0, text: 'é', hyperlink_id: null }],
      1,
    );
    expect(nfc.length).toBe(1);
    expect(nfc[0].width).toBe(1);

    const nfd = expandRunsToRow(
      [{ cols: 1, fg: null, bg: null, style: 0, text: 'é', hyperlink_id: null }],
      1,
    );
    expect(nfd.length).toBe(1);
    expect(nfd[0].width).toBe(1);
  });
});
