import { describe, expect, it } from 'vitest';
import type { FrameDelta, FrameSnapshot } from '../protocol/frame';
import { applyFrame, createGrid, expandRunsToRow } from './grid';

function snapshot(over: Partial<FrameSnapshot> = {}): FrameSnapshot {
  return {
    kind: 'snapshot',
    seq: 0,
    cols: 3,
    rows: 1,
    cursor: { x: 0, y: 0, shape: 'block', blinking: false, visible: true },
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
    hyperlinks: [],
    ...over,
  };
}

describe('createGrid', () => {
  it('initializes empty cells, no dirty rows, cursor at 0,0', () => {
    const g = createGrid({ cols: 80, rows: 24 });
    expect(g.cols).toBe(80);
    expect(g.rows).toBe(24);
    expect(g.cells.length).toBe(24);
    expect(g.cursor).toEqual({ x: 0, y: 0, shape: 'block', blinking: false, visible: true });
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

  it('preserves grid.modes Set identity across applySnapshot', () => {
    const g = createGrid({ cols: 80, rows: 24 });
    const initialModesRef = g.modes;
    applyFrame(g, snapshot({ modes: ['alt-screen', 'mouse-vt200'] }));
    expect(g.modes).toBe(initialModesRef);
    expect(g.modes.has('alt-screen')).toBe(true);
    expect(g.modes.has('mouse-vt200')).toBe(true);
  });

  it('clears previous modes on subsequent snapshot without identity change', () => {
    const g = createGrid({ cols: 80, rows: 24 });
    const initialModesRef = g.modes;
    applyFrame(g, snapshot({ modes: ['alt-screen'] }));
    applyFrame(g, snapshot({ modes: ['focus-events'] }));
    expect(g.modes).toBe(initialModesRef);
    expect(g.modes.has('alt-screen')).toBe(false);
    expect(g.modes.has('focus-events')).toBe(true);
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
      cursor: { x: 0, y: 0, shape: 'block', blinking: false, visible: true },
      dirty_rows: [
        {
          row: 1,
          runs: [{ cols: 3, fg: null, bg: null, style: 0, text: 'XYZ', hyperlink_id: null }],
        },
      ],
      hyperlinks: [],
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

  it('updates grid.cursor from delta cursor field', () => {
    const g = createGrid({ cols: 10, rows: 2 });
    g.cursor = { x: 0, y: 0, shape: 'block', blinking: false, visible: true };
    const delta: FrameDelta = {
      kind: 'delta',
      seq: 5,
      cursor: { x: 4, y: 1, shape: 'block', blinking: false, visible: true },
      dirty_rows: [],
      hyperlinks: [],
    };
    applyFrame(g, delta);
    expect(g.cursor).toEqual({ x: 4, y: 1, shape: 'block', blinking: false, visible: true });
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

describe('Grid.rowVersions and cellAtColumn', () => {
  it('rowVersions initializes to Uint32Array of length rows', () => {
    const g = createGrid({ cols: 80, rows: 24 });
    expect(g.rowVersions).toBeInstanceOf(Uint32Array);
    expect(g.rowVersions.length).toBe(24);
    expect(Array.from(g.rowVersions)).toEqual(Array(24).fill(0));
  });

  it('applyFrame snapshot resizes rowVersions and bumps all rows', () => {
    const g = createGrid({ cols: 80, rows: 24 });
    applyFrame(
      g,
      snapshot({
        cols: 2,
        rows: 3,
        rows_data: [{ runs: [] }, { runs: [] }, { runs: [] }],
      }),
    );
    expect(g.rowVersions.length).toBe(3);
    expect(Array.from(g.rowVersions)).toEqual([1, 1, 1]);
  });

  it('applyFrame delta bumps only dirty rows', () => {
    const g = createGrid({ cols: 3, rows: 3 });
    applyFrame(
      g,
      snapshot({
        cols: 3,
        rows: 3,
        rows_data: [{ runs: [] }, { runs: [] }, { runs: [] }],
      }),
    );
    const delta: FrameDelta = {
      kind: 'delta',
      seq: 2,
      cursor: { x: 0, y: 0, shape: 'block', blinking: false, visible: true },
      dirty_rows: [{ row: 1, runs: [] }],
      hyperlinks: [],
    };
    const beforeRef = g.rowVersions;
    applyFrame(g, delta);
    expect(Array.from(g.rowVersions)).toEqual([1, 2, 1]);
    // delta with at least one dirty row must replace the rowVersions typed
    // array so grid-store notifies subscribers (reference comparison).
    expect(g.rowVersions).not.toBe(beforeRef);
  });

  it('applyFrame delta with EMPTY dirty_rows keeps the rowVersions reference (cursor-only update)', () => {
    const g = createGrid({ cols: 3, rows: 3 });
    applyFrame(
      g,
      snapshot({
        cols: 3,
        rows: 3,
        rows_data: [{ runs: [] }, { runs: [] }, { runs: [] }],
      }),
    );
    const beforeRef = g.rowVersions;
    const delta: FrameDelta = {
      kind: 'delta',
      seq: 3,
      cursor: { x: 1, y: 0, shape: 'block', blinking: false, visible: true },
      dirty_rows: [],
      hyperlinks: [],
    };
    applyFrame(g, delta);
    // No dirty rows → no row version bumps → reference unchanged.
    expect(g.rowVersions).toBe(beforeRef);
  });

  it('cellAtColumn returns the cell at a given terminal column (wide-char aware)', () => {
    const g = createGrid({ cols: 5, rows: 1 });
    g.cells[0] = [
      { text: 'a', width: 1, fg: null, bg: null, style: 0 },
      { text: '日', width: 2, fg: null, bg: null, style: 0 },
      { text: 'z', width: 1, fg: null, bg: null, style: 0 },
    ];
    expect(g.cellAtColumn(0, 0)?.cell.text).toBe('a');
    expect(g.cellAtColumn(0, 0)?.startCol).toBe(0);
    expect(g.cellAtColumn(0, 0)?.endCol).toBe(1);
    expect(g.cellAtColumn(0, 1)?.cell.text).toBe('日');
    expect(g.cellAtColumn(0, 1)?.startCol).toBe(1);
    expect(g.cellAtColumn(0, 1)?.endCol).toBe(3);
    expect(g.cellAtColumn(0, 2)?.cell.text).toBe('日');
    expect(g.cellAtColumn(0, 3)?.cell.text).toBe('z');
    expect(g.cellAtColumn(0, 4)).toBeUndefined();
  });
});
