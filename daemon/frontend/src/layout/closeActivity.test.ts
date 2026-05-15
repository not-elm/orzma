import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { closeActivity } from './closeActivity';

const origFetch = globalThis.fetch;

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  globalThis.fetch = origFetch;
  vi.restoreAllMocks();
});

describe('closeActivity', () => {
  it('issues DELETE /windows/{wid}/panes/{pid}/activities/{aid}', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await closeActivity('wid-1', 'pid-42', 'aid-9');
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-42/activities/aid-9', {
      method: 'DELETE',
    });
  });

  it('warns on !ok status (e.g. 409 last-activity refused)', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 409 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await closeActivity('wid-1', 'pid-1', 'aid-2');
    expect(console.warn).toHaveBeenCalledWith(
      'close activity failed',
      expect.objectContaining({
        windowId: 'wid-1',
        paneId: 'pid-1',
        activityId: 'aid-2',
        status: 409,
      }),
    );
  });

  it('warns on network failure', async () => {
    const err = new Error('net');
    const fetchMock = vi.fn().mockRejectedValue(err);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await closeActivity('wid-1', 'pid-x', 'aid-y');
    expect(console.warn).toHaveBeenCalledWith(
      'close activity request errored',
      expect.objectContaining({
        windowId: 'wid-1',
        paneId: 'pid-x',
        activityId: 'aid-y',
        error: err,
      }),
    );
  });
});
