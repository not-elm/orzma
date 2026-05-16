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
});
