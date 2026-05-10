import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useDefaultWindow } from './useDefaultWindow';

const origFetch = global.fetch;

beforeEach(() => {
  global.fetch = vi.fn() as typeof global.fetch;
});
afterEach(() => {
  global.fetch = origFetch;
});

describe('useDefaultWindow', () => {
  it('returns ready with active_window of first session', async () => {
    (global.fetch as ReturnType<typeof vi.fn>).mockImplementation((url: string) => {
      if (url === '/sessions') {
        return Promise.resolve({
          ok: true,
          json: () => Promise.resolve({ sessions: [{ id: 'sid-1' }] }),
        } as Response);
      }
      if (url === '/sessions/sid-1') {
        return Promise.resolve({
          ok: true,
          json: () =>
            Promise.resolve({ windows: ['wid-a', 'wid-b'], active_window: 'wid-a' }),
        } as Response);
      }
      return Promise.reject(new Error(`unexpected ${url}`));
    });
    const { result } = renderHook(() => useDefaultWindow());
    await waitFor(() => expect(result.current.status).toBe('ready'));
    expect((result.current as { windowId: string }).windowId).toBe('wid-a');
  });

  it('falls back to first window when active_window is null', async () => {
    (global.fetch as ReturnType<typeof vi.fn>).mockImplementation((url: string) => {
      if (url === '/sessions')
        return Promise.resolve({
          ok: true,
          json: () => Promise.resolve({ sessions: [{ id: 'sid-1' }] }),
        } as Response);
      if (url === '/sessions/sid-1')
        return Promise.resolve({
          ok: true,
          json: () =>
            Promise.resolve({ windows: ['wid-a', 'wid-b'], active_window: null }),
        } as Response);
      return Promise.reject(new Error('unexpected'));
    });
    const { result } = renderHook(() => useDefaultWindow());
    await waitFor(() => expect(result.current.status).toBe('ready'));
    expect((result.current as { windowId: string }).windowId).toBe('wid-a');
  });

  it('returns error when no sessions exist', async () => {
    (global.fetch as ReturnType<typeof vi.fn>).mockImplementation((url: string) => {
      if (url === '/sessions')
        return Promise.resolve({
          ok: true,
          json: () => Promise.resolve({ sessions: [] }),
        } as Response);
      return Promise.reject(new Error('unexpected'));
    });
    const { result } = renderHook(() => useDefaultWindow());
    await waitFor(() => expect(result.current.status).toBe('error'));
  });
});
