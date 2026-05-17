import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useSessionTree } from './useSessionTree';

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

describe('useSessionTree', () => {
  it('returns ready with parsed sessions on success', async () => {
    globalThis.fetch = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          sessions: [
            {
              id: 'sid-a',
              name: 'work',
              active_window: 'wid-1',
              windows: [{ id: 'wid-1', name: 'alpha', index: 0 }],
            },
          ],
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    ) as typeof globalThis.fetch;

    const { result } = renderHook(() => useSessionTree(true));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    if (result.current.status !== 'ready') throw new Error('unreachable');
    expect(result.current.tree).toHaveLength(1);
    expect(result.current.tree[0]?.id).toBe('sid-a');
    expect(result.current.tree[0]?.windows[0]?.name).toBe('alpha');
  });

  it('returns error on non-OK', async () => {
    globalThis.fetch = vi.fn().mockResolvedValue(new Response(null, { status: 500 })) as typeof globalThis.fetch;
    const { result } = renderHook(() => useSessionTree(true));
    await waitFor(() => expect(result.current.status).toBe('error'));
  });

  it('does not fetch when active=false', () => {
    const fetchMock = vi.fn();
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    renderHook(() => useSessionTree(false));
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
