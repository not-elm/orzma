import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useDefaultSession } from './useDefaultSession';

const origFetch = globalThis.fetch;

beforeEach(() => {
  globalThis.fetch = vi.fn() as typeof globalThis.fetch;
});
afterEach(() => {
  globalThis.fetch = origFetch;
});

describe('useDefaultSession', () => {
  it('resolves to the first session id', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockImplementation(() =>
      Promise.resolve({
        ok: true,
        json: () => Promise.resolve({ sessions: [{ id: 'sid-0' }, { id: 'sid-1' }] }),
      } as Response),
    );

    const { result } = renderHook(() => useDefaultSession());
    await waitFor(() => expect(result.current).toEqual({ status: 'ready', sessionId: 'sid-0' }));
  });

  it('reports error when there are no sessions', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockImplementation(() =>
      Promise.resolve({
        ok: true,
        json: () => Promise.resolve({ sessions: [] }),
      } as Response),
    );

    const { result } = renderHook(() => useDefaultSession());
    await waitFor(() => expect(result.current.status).toBe('error'));
  });

  it('reports error when fetch rejects', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockImplementation(() =>
      Promise.reject(new Error('net')),
    );

    const { result } = renderHook(() => useDefaultSession());
    await waitFor(() => expect(result.current.status).toBe('error'));
  });
});
