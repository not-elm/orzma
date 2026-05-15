import { act, renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { createOverlayStore, OverlayStoreContext, useOverlayState } from './overlay-store';

const baseCursor = { x: 0, y: 0, shape: 'block' as const, blinking: false, visible: true };
const baseFm = { cellW: 8, cellH: 16, baseline: 12, fontCss: '14px monospace', dpr: 1 };

function withStore(store: ReturnType<typeof createOverlayStore>) {
  return ({ children }: { children: React.ReactNode }) => (
    <OverlayStoreContext.Provider value={store}>{children}</OverlayStoreContext.Provider>
  );
}

describe('overlay-store (per-Terminal factory)', () => {
  it('useOverlayState returns the current state via Context', () => {
    const store = createOverlayStore();
    store.setOverlayState({ cursor: baseCursor, cols: 80, rows: 24, fm: baseFm });
    const { result } = renderHook(() => useOverlayState(), { wrapper: withStore(store) });
    expect(result.current.cols).toBe(80);
    expect(result.current.cursor.x).toBe(0);
  });

  it('does not re-render subscribers when next state is shallow-equal', () => {
    const store = createOverlayStore();
    store.setOverlayState({ cursor: baseCursor, cols: 80, rows: 24, fm: baseFm });
    const { result, rerender } = renderHook(() => useOverlayState(), {
      wrapper: withStore(store),
    });
    const before = result.current;
    store.setOverlayState({ cursor: { ...baseCursor }, cols: 80, rows: 24, fm: baseFm });
    rerender();
    expect(result.current).toBe(before);
  });

  it('notifies subscribers when cursor.x changes', () => {
    const store = createOverlayStore();
    store.setOverlayState({ cursor: baseCursor, cols: 80, rows: 24, fm: baseFm });
    const { result, rerender } = renderHook(() => useOverlayState(), {
      wrapper: withStore(store),
    });
    const before = result.current;
    act(() =>
      store.setOverlayState({ cursor: { ...baseCursor, x: 5 }, cols: 80, rows: 24, fm: baseFm }),
    );
    rerender();
    expect(result.current).not.toBe(before);
    expect(result.current.cursor.x).toBe(5);
  });

  it('isolates state between two stores (multi-pane invariant)', () => {
    const a = createOverlayStore();
    const b = createOverlayStore();
    a.setOverlayState({ cursor: { ...baseCursor, x: 1 }, cols: 80, rows: 24, fm: baseFm });
    b.setOverlayState({ cursor: { ...baseCursor, x: 9 }, cols: 80, rows: 24, fm: baseFm });
    expect(a.getSnapshot().cursor.x).toBe(1);
    expect(b.getSnapshot().cursor.x).toBe(9);
  });

  it('useOverlayState throws when not inside an OverlayStoreContext.Provider', () => {
    expect(() => renderHook(() => useOverlayState())).toThrow();
  });
});
