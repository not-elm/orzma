import { act, renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { createGrid } from './grid';
import { createGridStore, GridStoreContext, useGridStore } from './grid-store';

function withStore(store: ReturnType<typeof createGridStore>) {
  return ({ children }: { children: React.ReactNode }) => (
    <GridStoreContext.Provider value={store}>{children}</GridStoreContext.Provider>
  );
}

describe('grid-store (per-Terminal factory)', () => {
  it('createGridStore exposes setGrid / subscribe / getSnapshot', () => {
    const store = createGridStore();
    expect(typeof store.setGrid).toBe('function');
    expect(typeof store.subscribe).toBe('function');
    expect(typeof store.getSnapshot).toBe('function');
  });

  it('useGridStore returns the store snapshot via Context', () => {
    const store = createGridStore();
    const g = createGrid({ cols: 80, rows: 24 });
    store.setGrid(g);
    const { result } = renderHook(() => useGridStore(), { wrapper: withStore(store) });
    expect(result.current.cols).toBe(80);
    expect(result.current.rows).toBe(24);
  });

  it('skips notification when rowVersions reference is unchanged', () => {
    const store = createGridStore();
    const g = createGrid({ cols: 80, rows: 24 });
    store.setGrid(g);
    const { result, rerender } = renderHook(() => useGridStore(), {
      wrapper: withStore(store),
    });
    const before = result.current;
    store.setGrid(g); // same reference
    rerender();
    expect(result.current).toBe(before);
  });

  it('notifies subscribers when rowVersions reference changes', () => {
    const store = createGridStore();
    const g1 = createGrid({ cols: 80, rows: 24 });
    store.setGrid(g1);
    const { result, rerender } = renderHook(() => useGridStore(), {
      wrapper: withStore(store),
    });
    const before = result.current;
    const g2 = { ...g1, rowVersions: new Uint32Array(24) };
    act(() => store.setGrid(g2));
    rerender();
    expect(result.current).not.toBe(before);
    expect(result.current.rowVersions).toBe(g2.rowVersions);
  });

  it('notifies subscribers when cursor.x changes', () => {
    const store = createGridStore();
    const g1 = createGrid({ cols: 80, rows: 24 });
    store.setGrid(g1);
    const { result, rerender } = renderHook(() => useGridStore(), {
      wrapper: withStore(store),
    });
    const before = result.current;
    const g2 = { ...g1, cursor: { ...g1.cursor, x: 5 } };
    act(() => store.setGrid(g2));
    rerender();
    expect(result.current).not.toBe(before);
    expect(result.current.cursor.x).toBe(5);
  });

  it('isolates state between two stores (multi-pane invariant)', () => {
    const a = createGridStore();
    const b = createGridStore();
    a.setGrid({
      ...createGrid({ cols: 80, rows: 24 }),
      cursor: { x: 1, y: 0, shape: 'block', blinking: false, visible: true },
    });
    b.setGrid({
      ...createGrid({ cols: 80, rows: 24 }),
      cursor: { x: 9, y: 0, shape: 'block', blinking: false, visible: true },
    });
    expect(a.getSnapshot().cursor.x).toBe(1);
    expect(b.getSnapshot().cursor.x).toBe(9);
  });

  it('useGridStore throws when not inside a GridStoreContext.Provider', () => {
    expect(() => renderHook(() => useGridStore())).toThrow();
  });
});
