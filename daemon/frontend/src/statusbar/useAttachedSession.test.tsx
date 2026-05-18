import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useAttachedSession } from './useAttachedSession';

const originalFetch = globalThis.fetch;
const originalLocation = window.location;

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
});

afterEach(() => {
  vi.useRealTimers();
  globalThis.fetch = originalFetch;
  Object.defineProperty(window, 'location', { value: originalLocation, configurable: true });
});

function mockLocation(search: string) {
  Object.defineProperty(window, 'location', {
    value: { ...originalLocation, search, href: `http://x/${search}` },
    configurable: true,
  });
}

function mockFetchSequence(responses: Array<{ sessions: Array<{ id: string }> }>) {
  let i = 0;
  globalThis.fetch = vi.fn(async () => {
    const body = responses[Math.min(i, responses.length - 1)];
    i++;
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  }) as unknown as typeof fetch;
}

describe('useAttachedSession', () => {
  it('no query attaches to first session', async () => {
    mockLocation('');
    mockFetchSequence([{ sessions: [{ id: 'sess-1' }, { id: 'sess-2' }] }]);
    const { result } = renderHook(() => useAttachedSession());
    await waitFor(() => expect(result.current.status).toBe('ready'));
    if (result.current.status === 'ready') {
      expect(result.current.sessionId).toBe('sess-1');
    }
  });

  it('existing deep-link attaches to that id immediately', async () => {
    mockLocation('?session=sess-2');
    mockFetchSequence([{ sessions: [{ id: 'sess-1' }, { id: 'sess-2' }] }]);
    const { result } = renderHook(() => useAttachedSession());
    await waitFor(() => expect(result.current.status).toBe('ready'));
    if (result.current.status === 'ready') {
      expect(result.current.sessionId).toBe('sess-2');
    }
  });

  it('late-arriving deep-link resolves on retry', async () => {
    mockLocation('?session=sess-late');
    mockFetchSequence([
      { sessions: [{ id: 'sess-1' }] },
      { sessions: [{ id: 'sess-1' }, { id: 'sess-late' }] },
    ]);
    const { result } = renderHook(() => useAttachedSession());
    await vi.advanceTimersByTimeAsync(250);
    await waitFor(() => expect(result.current.status).toBe('ready'));
    if (result.current.status === 'ready') {
      expect(result.current.sessionId).toBe('sess-late');
    }
  });

  it('unknown deep-link falls back to first with warning', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    mockLocation('?session=sess-never');
    mockFetchSequence([{ sessions: [{ id: 'sess-1' }] }]);
    const { result } = renderHook(() => useAttachedSession());
    await vi.advanceTimersByTimeAsync(800);
    await waitFor(() => expect(result.current.status).toBe('ready'));
    if (result.current.status === 'ready') {
      expect(result.current.sessionId).toBe('sess-1');
    }
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});
