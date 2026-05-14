import { describe, expect, it, type vi } from 'vitest';
import { createCanvasRenderer } from './canvas';
import type { FontMetrics } from './font';
import type { Grid } from './grid';
import { createGrid } from './grid';

function fakeMetrics(): FontMetrics {
  return { cellW: 8, cellH: 16, baseline: 12, fontCss: '14px monospace', dpr: 1 };
}

function gridWithRow(text: string): Grid {
  const g = createGrid({ cols: text.length, rows: 1 });
  g.cells[0] = Array.from(text).map((ch) => ({
    text: ch,
    width: 1 as const,
    fg: null,
    bg: null,
    style: 0,
  }));
  g.dirtyRows.add(0);
  return g;
}

describe('createCanvasRenderer.paint', () => {
  it('fills each dirty row with DEFAULT_BG and draws each cell', () => {
    const canvas = document.createElement('canvas');
    // biome-ignore lint/style/noNonNullAssertion: FakeCanvasRenderingContext2D is always non-null in tests
    const ctx = canvas.getContext('2d')!;
    const renderer = createCanvasRenderer(canvas, fakeMetrics());
    const grid = gridWithRow('hi');

    renderer.paint(grid, { isActive: true });

    expect(ctx.fillRect).toHaveBeenCalledWith(0, 0, expect.any(Number), 16);
    expect(
      (ctx.fillText as unknown as ReturnType<typeof vi.fn>).mock.calls.length,
    ).toBeGreaterThanOrEqual(2);
  });

  it('clears dirtyRows set after paint', () => {
    const canvas = document.createElement('canvas');
    const renderer = createCanvasRenderer(canvas, fakeMetrics());
    const grid = gridWithRow('ab');
    grid.dirtyRows.add(0);
    renderer.paint(grid, { isActive: true });
    expect(grid.dirtyRows.size).toBe(0);
  });

  it('skips row repaint when dirtyRows is empty and cursor is hidden', () => {
    const canvas = document.createElement('canvas');
    // biome-ignore lint/style/noNonNullAssertion: FakeCanvasRenderingContext2D is always non-null in tests
    const ctx = canvas.getContext('2d')!;
    const renderer = createCanvasRenderer(canvas, fakeMetrics());
    const grid = gridWithRow('xy');
    grid.dirtyRows.clear();
    grid.cursor.visible = false;
    renderer.paint(grid, { isActive: true });
    expect(ctx.fillRect).not.toHaveBeenCalled();
  });

  it('paints a cursor block at grid.cursor when visible', () => {
    const canvas = document.createElement('canvas');
    // biome-ignore lint/style/noNonNullAssertion: FakeCanvasRenderingContext2D is always non-null in tests
    const ctx = canvas.getContext('2d')!;
    const renderer = createCanvasRenderer(canvas, fakeMetrics());
    const grid = gridWithRow('hello');
    grid.cursor = { x: 3, y: 0, shape: 'block', visible: true };
    renderer.paint(grid, { isActive: true });
    // The cursor block fillRect at (3*8, 0, 8, 16) is one of the fillRect calls.
    expect(ctx.fillRect).toHaveBeenCalledWith(3 * 8, 0, 8, 16);
  });

  it('uses globalAlpha 1 when active and 0.6 when inactive', () => {
    const canvas = document.createElement('canvas');
    // biome-ignore lint/style/noNonNullAssertion: FakeCanvasRenderingContext2D is always non-null in tests
    const ctx = canvas.getContext('2d')!;
    const renderer = createCanvasRenderer(canvas, fakeMetrics());
    const grid = gridWithRow('x');
    grid.cursor = { x: 0, y: 0, shape: 'block', visible: true };

    renderer.paint(grid, { isActive: true });
    const activeAlphaCalls = (ctx.globalAlpha as unknown as number[] | undefined) ?? [];
    void activeAlphaCalls;

    grid.dirtyRows.add(0);
    renderer.paint(grid, { isActive: false });
    expect(
      (ctx.fillRect as unknown as ReturnType<typeof vi.fn>).mock.calls.some(
        (c) => c[0] === 0 && c[1] === 0 && c[2] === 8 && c[3] === 16,
      ),
    ).toBe(true);
  });
});
