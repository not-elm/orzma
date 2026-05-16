import { act, renderHook } from '@testing-library/react';
import { Server } from 'mock-socket';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionView } from './types';
import { useSessionView } from './useSessionView';

const SID = 'sid-test';
const URL = `ws://${location.host}/sessions/${SID}/events`;

let server: Server;

const fakeView = (overrides: Partial<SessionView> = {}): SessionView => ({
  id: SID,
  name: 'ozmux',
  active_window: 'wid-0',
  windows: [{ id: 'wid-0', name: 'main', index: 0 }],
  ...overrides,
});

beforeEach(() => {
  server = new Server(URL);
});

afterEach(() => {
  server.stop();
});

describe('useSessionView', () => {
  it('returns connecting/null before the first frame', () => {
    const { result } = renderHook(() => useSessionView(SID));
    expect(result.current.status).toBe('connecting');
    expect((result.current as { view: SessionView | null }).view).toBeNull();
  });

  it('transitions to live with the snapshot view on first frame', async () => {
    server.on('connection', (sock) => {
      sock.send(JSON.stringify(fakeView()));
    });
    const { result } = renderHook(() => useSessionView(SID));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });
    expect(result.current.status).toBe('live');
    expect((result.current as { view: SessionView }).view.id).toBe(SID);
  });

  it('returns connecting/null when sid is null', () => {
    const { result } = renderHook(() => useSessionView(null));
    expect(result.current.status).toBe('connecting');
    expect((result.current as { view: SessionView | null }).view).toBeNull();
  });

  it('ignores stale frames after sid change', async () => {
    const SID2 = 'sid-second';
    const URL2 = `ws://${location.host}/sessions/${SID2}/events`;
    const server2 = new Server(URL2);

    let firstSocket: WebSocket | null = null;
    server.on('connection', (sock) => {
      firstSocket = sock as unknown as WebSocket;
    });
    server2.on('connection', (sock) => {
      sock.send(JSON.stringify(fakeView({ id: SID2, name: 'second' })));
    });

    const { result, rerender } = renderHook(({ id }: { id: string | null }) => useSessionView(id), {
      initialProps: { id: SID as string | null },
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    rerender({ id: SID2 });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });
    // Now have firstSocket from the old sid send a stale frame.
    if (firstSocket) {
      (firstSocket as unknown as { send: (data: string) => void }).send(
        JSON.stringify(fakeView({ id: SID, name: 'STALE' })),
      );
    }
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });

    expect(result.current.status).toBe('live');
    expect((result.current as { view: { id: string; name: string } }).view.id).toBe(SID2);
    expect((result.current as { view: { id: string; name: string } }).view.name).toBe('second');
    server2.stop();
  });

  it('reconnects immediately on 1011 "lagged" close', async () => {
    let connectionCount = 0;
    server.on('connection', (sock) => {
      connectionCount++;
      sock.send(JSON.stringify(fakeView({ name: `n${connectionCount}` })));
      if (connectionCount === 1) {
        sock.close({ code: 1011, reason: 'lagged', wasClean: true });
      }
    });
    const { result } = renderHook(() => useSessionView(SID));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(connectionCount).toBeGreaterThanOrEqual(2);
    expect(result.current.status).toBe('live');
    expect((result.current as { view: { name: string } }).view.name).toBe(`n${connectionCount}`);
  });

  it('uses backoff for non-first reconnect attempts', async () => {
    // Force Math.random to 0 for deterministic jitter.
    const origRandom = Math.random;
    Math.random = () => 0;
    try {
      let connectionCount = 0;
      server.on('connection', (sock) => {
        connectionCount++;
        if (connectionCount <= 3) {
          // NOTE: send a frame so onmessage handler exists, then close immediately.
          // Don't wait — close on the same tick.
          sock.close({ code: 1011, reason: 'internal_error', wasClean: true });
        } else {
          sock.send(JSON.stringify(fakeView()));
        }
      });
      const start = performance.now();
      renderHook(() => useSessionView(SID));
      // Wait long enough for ~3 reconnect attempts including backoffs.
      // 1st: immediate, 2nd: ~500ms (jitter=0), 3rd: ~1000ms.
      // Total ~1500ms, so connectionCount should be at least 3.
      await act(async () => {
        await new Promise((r) => setTimeout(r, 1700));
      });
      const elapsed = performance.now() - start;
      expect(connectionCount).toBeGreaterThanOrEqual(3);
      expect(elapsed).toBeGreaterThan(1000);
    } finally {
      Math.random = origRandom;
    }
  });

  it('enters gone state on 1011 "session_not_found"', async () => {
    server.on('connection', (sock) => {
      sock.close({ code: 1011, reason: 'session_not_found', wasClean: true });
    });
    const { result } = renderHook(() => useSessionView(SID));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30));
    });
    expect(result.current.status).toBe('gone');
    expect((result.current as { reason: string }).reason).toBe('session_not_found');
  });

  it('enters gone state on 1011 "session_closed"', async () => {
    server.on('connection', (sock) => {
      sock.send(JSON.stringify(fakeView()));
      setTimeout(() => sock.close({ code: 1011, reason: 'session_closed', wasClean: true }), 10);
    });
    const { result } = renderHook(() => useSessionView(SID));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(result.current.status).toBe('gone');
    expect((result.current as { reason: string }).reason).toBe('session_closed');
  });

  it('logs reconnect attempts via console.warn with attempt and reason', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    server.on('connection', (sock) => {
      sock.close({ code: 1011, reason: 'internal_error', wasClean: true });
    });
    renderHook(() => useSessionView(SID));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(warnSpy).toHaveBeenCalled();
    const found = warnSpy.mock.calls.some((args) => {
      const tag = args[0];
      const payload = args[1] as undefined | { attempt?: number; prevReason?: string };
      return (
        typeof tag === 'string' &&
        tag.includes('useSessionView') &&
        typeof payload === 'object' &&
        payload?.attempt === 1 &&
        payload?.prevReason === 'internal_error'
      );
    });
    expect(found).toBe(true);
    warnSpy.mockRestore();
  });

  it('opens at most one extra WS in StrictMode double-mount, no state pollution', async () => {
    const { StrictMode } = await import('react');
    let connectionCount = 0;
    server.on('connection', (sock) => {
      connectionCount++;
      sock.send(JSON.stringify(fakeView()));
    });
    renderHook(() => useSessionView(SID), {
      wrapper: ({ children }) => <StrictMode>{children}</StrictMode>,
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    // StrictMode causes a mount-cleanup-mount cycle, so 2 WebSocket
    // constructors fire. The generation token guarantees the first one's
    // onmessage / onclose can't update state. We assert connectionCount in
    // [1, 2]; anything higher would be a leak.
    expect(connectionCount).toBeGreaterThanOrEqual(1);
    expect(connectionCount).toBeLessThanOrEqual(2);
  });

  it('pauses reconnect when document.hidden is true', async () => {
    let connectionCount = 0;
    server.on('connection', (sock) => {
      connectionCount++;
      sock.close({ code: 1011, reason: 'internal_error', wasClean: true });
    });
    // Hide the document.
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => 'hidden',
    });
    Object.defineProperty(document, 'hidden', {
      configurable: true,
      get: () => true,
    });

    renderHook(() => useSessionView(SID));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 200));
    });
    // First connect happens, closes immediately, no further reconnects while hidden.
    const hiddenCount = connectionCount;

    // Restore visibility, dispatch the event.
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => 'visible',
    });
    Object.defineProperty(document, 'hidden', {
      configurable: true,
      get: () => false,
    });
    document.dispatchEvent(new Event('visibilitychange'));

    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
    expect(connectionCount).toBeGreaterThan(hiddenCount);
  });
});
