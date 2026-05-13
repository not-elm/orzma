import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { actionToHandler, type ShortcutContext } from './actionDispatch';
import type { Action } from './wire';

const origFetch = globalThis.fetch;

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  globalThis.fetch = origFetch;
  vi.restoreAllMocks();
});

describe('actionToHandler', () => {
  it('returns a handler that calls closePane on close-pane', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx: ShortcutContext = {
      activeWindow: () => 'wid-1',
      activePane: () => 'pid-1',
    };
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
    const ctx: ShortcutContext = {
      activeWindow: () => null,
      activePane: () => null,
    };
    const handler = actionToHandler({ type: 'close-pane' }, ctx);
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('returns a handler that POSTs split with horizontal orientation', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 201 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx: ShortcutContext = {
      activeWindow: () => 'wid-1',
      activePane: () => 'pid-1',
    };
    const handler = actionToHandler(
      { type: 'split-pane', direction: 'horizontal' },
      ctx,
    );
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-1/split', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ orientation: 'horizontal' }),
    });
  });

  it('returns a handler that POSTs split with vertical orientation', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 201 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx: ShortcutContext = {
      activeWindow: () => 'wid-1',
      activePane: () => 'pid-1',
    };
    const handler = actionToHandler(
      { type: 'split-pane', direction: 'vertical' },
      ctx,
    );
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-1/split', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ orientation: 'vertical' }),
    });
  });

  it('split-pane handler is a no-op when active pane is null', async () => {
    const fetchMock = vi.fn();
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    const ctx: ShortcutContext = {
      activeWindow: () => null,
      activePane: () => null,
    };
    const handler = actionToHandler(
      { type: 'split-pane', direction: 'horizontal' },
      ctx,
    );
    if (handler === null) {
      throw new Error('handler should not be null');
    }
    handler();
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('returns null and warns for unknown action types', () => {
    const ctx: ShortcutContext = { activeWindow: () => null, activePane: () => null };
    const handler = actionToHandler({ type: 'totally-unknown' } as unknown as Action, ctx);
    expect(handler).toBeNull();
    expect(console.warn).toHaveBeenCalled();
  });
});
