import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useAttachedSession } from './useAttachedSession';

const origFetch = globalThis.fetch;

beforeEach(() => {
  globalThis.fetch = vi.fn() as typeof globalThis.fetch;
});
afterEach(() => {
  globalThis.fetch = origFetch;
});

describe('useAttachedSession', () => {
  it('resolves to the first session id', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockImplementation(() =>
      Promise.resolve({
        ok: true,
        json: () => Promise.resolve({ sessions: [{ id: 'sid-0' }, { id: 'sid-1' }] }),
      } as Response),
    );

    const { result } = renderHook(() => useAttachedSession());
    await waitFor(() => expect(result.current.status).toBe('ready'));
    if (result.current.status !== 'ready') throw new Error('unreachable');
    expect(result.current.sessionId).toBe('sid-0');
  });

  it('reports error when there are no sessions', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockImplementation(() =>
      Promise.resolve({
        ok: true,
        json: () => Promise.resolve({ sessions: [] }),
      } as Response),
    );

    const { result } = renderHook(() => useAttachedSession());
    await waitFor(() => expect(result.current.status).toBe('error'));
  });

  it('reports error when fetch rejects', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockImplementation(() =>
      Promise.reject(new Error('net')),
    );

    const { result } = renderHook(() => useAttachedSession());
    await waitFor(() => expect(result.current.status).toBe('error'));
  });

  it('setSession switches the returned sessionId', async () => {
    globalThis.fetch = vi
      .fn()
      .mockResolvedValue(
        new Response(JSON.stringify({ sessions: [{ id: 'sid-a' }] }), { status: 200 }),
      ) as typeof globalThis.fetch;

    const { result } = renderHook(() => useAttachedSession());
    await waitFor(() => expect(result.current.status).toBe('ready'));
    if (result.current.status !== 'ready') throw new Error('unreachable');
    expect(result.current.sessionId).toBe('sid-a');

    act(() => {
      if (result.current.status === 'ready') result.current.setSession('sid-b');
    });
    if (result.current.status !== 'ready') throw new Error('unreachable');
    expect(result.current.sessionId).toBe('sid-b');
  });
});
