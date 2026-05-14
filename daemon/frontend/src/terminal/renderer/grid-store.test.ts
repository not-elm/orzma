import { renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { createGrid } from './grid';
import { setGrid, useGridStore } from './grid-store';

describe('grid-store', () => {
  it('useGridStore returns the current grid', () => {
    const g = createGrid({ cols: 80, rows: 24 });
    setGrid(g);
    const { result } = renderHook(() => useGridStore());
    expect(result.current.cols).toBe(80);
    expect(result.current.rows).toBe(24);
  });

  it('skips notification when rowVersions reference is unchanged', () => {
    const g = createGrid({ cols: 80, rows: 24 });
    setGrid(g);
    const { result, rerender } = renderHook(() => useGridStore());
    const before = result.current;
    // same Grid object → reference equal → skip
    setGrid(g);
    rerender();
    expect(result.current).toBe(before);
  });

  it('notifies subscribers when rowVersions reference changes', () => {
    const g1 = createGrid({ cols: 80, rows: 24 });
    setGrid(g1);
    const { result, rerender } = renderHook(() => useGridStore());
    const before = result.current;

    const g2 = { ...g1, rowVersions: new Uint32Array(24) };
    setGrid(g2);
    rerender();
    expect(result.current).not.toBe(before);
    expect(result.current.rowVersions).toBe(g2.rowVersions);
  });

  it('notifies subscribers when cursor.x changes', () => {
    const g1 = createGrid({ cols: 80, rows: 24 });
    setGrid(g1);
    const { result, rerender } = renderHook(() => useGridStore());
    const before = result.current;

    const g2 = { ...g1, cursor: { ...g1.cursor, x: 5 } };
    setGrid(g2);
    rerender();
    expect(result.current).not.toBe(before);
    expect(result.current.cursor.x).toBe(5);
  });
});
