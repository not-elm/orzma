import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { makeShortcutContext } from './__test-helpers';
import { actionToHandler } from './actionDispatch';
import type { Action } from './wire';

const origFetch = globalThis.fetch;

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  globalThis.fetch = origFetch;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('actionToHandler', () => {
  it('returns a handler that calls closePane on close-pane', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext({
      activeWindow: () => 'wid-1',
      activePane: () => 'pid-1',
    });
    const handler = actionToHandler({ type: 'close-pane' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    // closePane is async fire-and-forget; flush microtasks
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-1', { method: 'DELETE' });
  });

  it('returns a no-op handler when context lacks an active pane', async () => {
    const fetchMock = vi.fn();
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext();
    const handler = actionToHandler({ type: 'close-pane' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each([
    'horizontal',
    'vertical',
  ] as const)('returns a handler that POSTs split with %s orientation', async (orientation) => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 201 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext({
      activeWindow: () => 'wid-1',
      activePane: () => 'pid-1',
    });
    const handler = actionToHandler({ type: 'split-pane', direction: orientation }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-1/split', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ orientation }),
    });
  });

  it('split-pane handler is a no-op when active pane is null', async () => {
    const fetchMock = vi.fn();
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext();
    const handler = actionToHandler({ type: 'split-pane', direction: 'horizontal' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('returns null and warns for unknown action types', () => {
    const ctx = makeShortcutContext();
    const handler = actionToHandler({ type: 'totally-unknown' } as unknown as Action, ctx);
    expect(handler).toBeNull();
    expect(console.warn).toHaveBeenCalled();
  });

  it('returns a handler that POSTs add then activate for new-terminal-activity', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce({ ok: true, status: 201 } as Response)
      .mockResolvedValueOnce({ ok: true, status: 204 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    vi.stubGlobal('crypto', {
      ...globalThis.crypto,
      randomUUID: () => 'aid-stub',
    });
    const ctx = makeShortcutContext({
      activeWindow: () => 'wid-1',
      activePane: () => 'pid-1',
    });
    const handler = actionToHandler({ type: 'new-terminal-activity' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith(
      '/windows/wid-1/panes/pid-1/activities',
      expect.objectContaining({ method: 'POST' }),
    );
    expect(fetchMock).toHaveBeenCalledWith(
      '/windows/wid-1/panes/pid-1/activities/aid-stub/activate',
      { method: 'POST' },
    );
  });

  it('new-terminal-activity handler is a no-op when active pane is null', async () => {
    const fetchMock = vi.fn();
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext();
    const handler = actionToHandler({ type: 'new-terminal-activity' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('returns a handler that DELETEs the activity for close-activity', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext({
      activeWindow: () => 'wid-1',
      activePane: () => 'pid-1',
      activeActivity: () => 'aid-9',
    });
    const handler = actionToHandler({ type: 'close-activity' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-1/activities/aid-9', {
      method: 'DELETE',
    });
  });

  it('returns a handler that POSTs break-to-pane with vertical orientation', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 201 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext({
      activeWindow: () => 'wid-1',
      activePane: () => 'pid-1',
      activeActivity: () => 'aid-1',
    });
    const handler = actionToHandler({ type: 'break-activity-to-pane', direction: 'vertical' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith(
      '/windows/wid-1/panes/pid-1/activities/aid-1/break-to-pane',
      {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ orientation: 'vertical' }),
      },
    );
  });

  it('close-activity handler is a no-op when active activity is null', async () => {
    const fetchMock = vi.fn();
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext({
      activeWindow: () => 'wid-1',
      activePane: () => 'pid-1',
      activeActivity: () => null,
    });
    const handler = actionToHandler({ type: 'close-activity' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each([
    'next',
    'prev',
  ] as const)('returns a handler that POSTs cycle-activity with %s direction', async (direction) => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext({
      activeWindow: () => 'wid-1',
      activePane: () => 'pid-1',
    });
    const handler = actionToHandler({ type: 'focus-activity', offset: direction }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-1/cycle-activity', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ direction }),
    });
  });

  it('focus-activity handler is a no-op when active pane is null', async () => {
    const fetchMock = vi.fn();
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext();
    const handler = actionToHandler({ type: 'focus-activity', offset: 'next' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each([
    'up',
    'down',
    'left',
    'right',
  ] as const)('returns a handler that POSTs focus-pane with %s direction', async (direction) => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext({
      activeWindow: () => 'wid-1',
    });
    const handler = actionToHandler({ type: 'focus-pane', direction }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/focus-pane', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ direction }),
    });
  });

  it('focus-pane handler is a no-op when active window is null', async () => {
    const fetchMock = vi.fn();
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext({ activeWindow: () => null });
    const handler = actionToHandler({ type: 'focus-pane', direction: 'left' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each([
    'left',
    'right',
    'up',
    'down',
  ] as const)('returns a handler that POSTs resize with %s direction', async (direction) => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext({
      activeWindow: () => 'wid-1',
      activePane: () => 'pid-1',
    });
    const handler = actionToHandler({ type: 'resize-pane', direction }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/windows/wid-1/panes/pid-1/resize');
    expect((init as RequestInit).method).toBe('POST');
    const body = JSON.parse((init as RequestInit).body as string);
    expect(body).toEqual({ direction, amount: 1 });
  });

  it('returns a no-op resize handler when context lacks an active pane', async () => {
    const fetchMock = vi.fn();
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext();
    const handler = actionToHandler({ type: 'resize-pane', direction: 'right' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  describe('window navigation', () => {
    function viewWithWindows(
      active: string,
      windows: Array<{ id: string; name: string; index: number }>,
    ) {
      return { id: 'sid-0', name: 's', active_window: active, windows };
    }

    it('focus-window next wraps to first', async () => {
      const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
      globalThis.fetch = fetchMock as typeof globalThis.fetch;
      const view = viewWithWindows('wid-1', [
        { id: 'wid-0', name: 'a', index: 0 },
        { id: 'wid-1', name: 'b', index: 1 },
      ]);
      const ctx = makeShortcutContext({ activeSession: () => view });
      const handler = actionToHandler({ type: 'focus-window', offset: 'next' }, ctx);
      handler?.();
      await Promise.resolve();
      await Promise.resolve();
      expect(fetchMock).toHaveBeenCalledWith('/windows/wid-0/select', { method: 'POST' });
    });

    it('focus-window prev wraps to last', async () => {
      const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
      globalThis.fetch = fetchMock as typeof globalThis.fetch;
      const view = viewWithWindows('wid-0', [
        { id: 'wid-0', name: 'a', index: 0 },
        { id: 'wid-1', name: 'b', index: 1 },
      ]);
      const ctx = makeShortcutContext({ activeSession: () => view });
      const handler = actionToHandler({ type: 'focus-window', offset: 'prev' }, ctx);
      handler?.();
      await Promise.resolve();
      await Promise.resolve();
      expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/select', { method: 'POST' });
    });

    it('focus-window-number selects by index', async () => {
      const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
      globalThis.fetch = fetchMock as typeof globalThis.fetch;
      const view = viewWithWindows('wid-0', [
        { id: 'wid-0', name: 'a', index: 0 },
        { id: 'wid-1', name: 'b', index: 1 },
      ]);
      const ctx = makeShortcutContext({ activeSession: () => view });
      const handler = actionToHandler({ type: 'focus-window-number', index: 1 }, ctx);
      handler?.();
      await Promise.resolve();
      await Promise.resolve();
      expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/select', { method: 'POST' });
    });

    it('focus-window-number with out-of-range index is no-op', async () => {
      const fetchMock = vi.fn();
      globalThis.fetch = fetchMock as typeof globalThis.fetch;
      const view = viewWithWindows('wid-0', [{ id: 'wid-0', name: 'a', index: 0 }]);
      const ctx = makeShortcutContext({ activeSession: () => view });
      const handler = actionToHandler({ type: 'focus-window-number', index: 5 }, ctx);
      handler?.();
      await Promise.resolve();
      expect(fetchMock).not.toHaveBeenCalled();
    });

    it('focus-window with single window is no-op', async () => {
      const fetchMock = vi.fn();
      globalThis.fetch = fetchMock as typeof globalThis.fetch;
      const view = viewWithWindows('wid-0', [{ id: 'wid-0', name: 'a', index: 0 }]);
      const ctx = makeShortcutContext({ activeSession: () => view });
      const handler = actionToHandler({ type: 'focus-window', offset: 'next' }, ctx);
      handler?.();
      await Promise.resolve();
      expect(fetchMock).not.toHaveBeenCalled();
    });
  });

  it('returns a handler that calls openRenameWindow on rename-window', () => {
    const openRenameWindow = vi.fn();
    const ctx = makeShortcutContext({ openRenameWindow });
    const handler = actionToHandler({ type: 'rename-window' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    expect(openRenameWindow).toHaveBeenCalledTimes(1);
  });

  it('returns a handler that POSTs /windows with the active session id', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 201 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const view = { id: 'sid-0', name: 's', active_window: 'wid-0', windows: [] };
    const ctx = makeShortcutContext({ activeSession: () => view });
    const handler = actionToHandler({ type: 'new-window' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith('/windows', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ session_id: 'sid-0' }),
    });
  });

  it('new-window handler is a no-op when there is no active session', async () => {
    const fetchMock = vi.fn();
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx = makeShortcutContext();
    const handler = actionToHandler({ type: 'new-window' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
