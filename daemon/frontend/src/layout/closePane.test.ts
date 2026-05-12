import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { closePane } from './closePane';

const origFetch = globalThis.fetch;

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  globalThis.fetch = origFetch;
  vi.restoreAllMocks();
});

describe('closePane', () => {
  it('issues DELETE /windows/{wid}/panes/{pid}', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await closePane('wid-1', 'pid-42');
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-42', { method: 'DELETE' });
  });

  it('warns on 409 (last pane) with windowId, paneId and status in the payload', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 409 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await closePane('wid-1', 'pid-1');
    expect(console.warn).toHaveBeenCalledWith(
      'close pane failed',
      expect.objectContaining({ windowId: 'wid-1', paneId: 'pid-1', status: 409 }),
    );
  });

  it('warns on 404 with windowId, paneId and status in the payload', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 404 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await closePane('wid-1', 'pid-gone');
    expect(console.warn).toHaveBeenCalledWith(
      'close pane failed',
      expect.objectContaining({ windowId: 'wid-1', paneId: 'pid-gone', status: 404 }),
    );
  });

  it('warns on network failure with windowId, paneId and error in the payload', async () => {
    const err = new Error('net');
    const fetchMock = vi.fn().mockRejectedValue(err);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await closePane('wid-1', 'pid-x');
    expect(console.warn).toHaveBeenCalledWith(
      'close pane request errored',
      expect.objectContaining({ windowId: 'wid-1', paneId: 'pid-x', error: err }),
    );
  });
});
