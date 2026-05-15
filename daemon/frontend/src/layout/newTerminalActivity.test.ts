import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { newTerminalActivity } from './newTerminalActivity';

const origFetch = globalThis.fetch;
const origRandomUUID = globalThis.crypto.randomUUID;

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  vi.stubGlobal('crypto', {
    ...globalThis.crypto,
    randomUUID: () => 'aid-stub-uuid',
  });
});

afterEach(() => {
  globalThis.fetch = origFetch;
  vi.unstubAllGlobals();
  globalThis.crypto.randomUUID = origRandomUUID;
  vi.restoreAllMocks();
});

describe('newTerminalActivity', () => {
  it('POSTs add then activate on the happy path', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 201 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await newTerminalActivity('wid-1', 'pid-1');
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(fetchMock).toHaveBeenNthCalledWith(1, '/windows/wid-1/panes/pid-1/activities', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        activity: { activity_id: 'aid-stub-uuid', kind: { type: 'terminal' } },
      }),
    });
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      '/windows/wid-1/panes/pid-1/activities/aid-stub-uuid/activate',
      { method: 'POST' },
    );
  });

  it('does not POST activate when add fails with non-ok status', async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce({ ok: false, status: 500 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await newTerminalActivity('wid-1', 'pid-1');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(console.warn).toHaveBeenCalledWith(
      'new terminal activity failed',
      expect.objectContaining({ windowId: 'wid-1', paneId: 'pid-1', status: 500 }),
    );
  });

  it('does not POST activate when add throws', async () => {
    const err = new Error('net');
    const fetchMock = vi.fn().mockRejectedValue(err);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await newTerminalActivity('wid-1', 'pid-1');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(console.warn).toHaveBeenCalledWith(
      'new terminal activity request errored',
      expect.objectContaining({ windowId: 'wid-1', paneId: 'pid-1', error: err }),
    );
  });

  it('warns but does not throw when activate fails after add succeeded', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce({ ok: true, status: 201 } as Response)
      .mockResolvedValueOnce({ ok: false, status: 404 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await newTerminalActivity('wid-1', 'pid-1');
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(console.warn).toHaveBeenCalledWith(
      'activate new activity failed',
      expect.objectContaining({
        windowId: 'wid-1',
        paneId: 'pid-1',
        activityId: 'aid-stub-uuid',
        status: 404,
      }),
    );
  });
});
