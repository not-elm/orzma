import { renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { setOverlayState, useOverlayState } from './overlay-store';

const baseCursor = { x: 0, y: 0, shape: 'block' as const, blinking: false, visible: true };
const baseFm = { cellW: 8, cellH: 16, baseline: 12, fontCss: '14px monospace', dpr: 1 };

describe('overlay-store', () => {
  it('useOverlayState returns the current state', () => {
    setOverlayState({ cursor: baseCursor, cols: 80, rows: 24, fm: baseFm });
    const { result } = renderHook(() => useOverlayState());
    expect(result.current.cols).toBe(80);
    expect(result.current.cursor.x).toBe(0);
  });

  it('does not re-render subscribers when next state is shallow-equal', () => {
    setOverlayState({ cursor: baseCursor, cols: 80, rows: 24, fm: baseFm });
    const { result, rerender } = renderHook(() => useOverlayState());
    const before = result.current;
    setOverlayState({ cursor: { ...baseCursor }, cols: 80, rows: 24, fm: baseFm });
    rerender();
    expect(result.current).toBe(before);
  });

  it('notifies subscribers when cursor.x changes', () => {
    setOverlayState({ cursor: baseCursor, cols: 80, rows: 24, fm: baseFm });
    const { result, rerender } = renderHook(() => useOverlayState());
    const before = result.current;
    setOverlayState({ cursor: { ...baseCursor, x: 5 }, cols: 80, rows: 24, fm: baseFm });
    rerender();
    expect(result.current).not.toBe(before);
    expect(result.current.cursor.x).toBe(5);
  });
});
