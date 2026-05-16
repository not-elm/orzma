import { renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { WindowId } from './types';
import { useWindowDimensions } from './useWindowDimensions';

class FakeResizeObserver {
  static cb: ResizeObserverCallback | null = null;
  constructor(cb: ResizeObserverCallback) {
    FakeResizeObserver.cb = cb;
  }
  observe() {}
  disconnect() {}
  unobserve() {}
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useWindowDimensions', () => {
  it('PATCHes once on mount without debouncing', () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal('fetch', fetchMock);
    vi.stubGlobal('ResizeObserver', FakeResizeObserver as unknown as typeof ResizeObserver);
    const el = document.createElement('div');
    Object.defineProperty(el, 'clientWidth', { value: 1200 });
    Object.defineProperty(el, 'clientHeight', { value: 400 });
    renderHook(() => useWindowDimensions('w1' as WindowId, el, { cellWidth: 10, cellHeight: 20 }));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const body = JSON.parse(fetchMock.mock.calls[0][1].body as string);
    expect(body).toEqual({ cols: 120, rows: 20 });
  });
});
