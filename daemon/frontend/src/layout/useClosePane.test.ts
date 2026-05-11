import { renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useClosePane } from './useClosePane';

const origFetch = globalThis.fetch;

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  globalThis.fetch = origFetch;
  vi.restoreAllMocks();
});

describe('useClosePane', () => {
  it('issues DELETE /panes/{id}', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const { result } = renderHook(() => useClosePane());
    await result.current('pid-42');
    expect(fetchMock).toHaveBeenCalledWith('/panes/pid-42', { method: 'DELETE' });
  });

  it('warns on 409 (last pane)', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 409 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const { result } = renderHook(() => useClosePane());
    await result.current('pid-1');
    expect(console.warn).toHaveBeenCalled();
  });

  it('warns on 404', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 404 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const { result } = renderHook(() => useClosePane());
    await result.current('pid-gone');
    expect(console.warn).toHaveBeenCalled();
  });

  it('warns on network failure', async () => {
    const fetchMock = vi.fn().mockRejectedValue(new Error('net'));
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const { result } = renderHook(() => useClosePane());
    await result.current('pid-x');
    expect(console.warn).toHaveBeenCalled();
  });
});
