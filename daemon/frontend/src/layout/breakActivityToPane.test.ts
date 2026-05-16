import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { breakActivityToPane } from './breakActivityToPane';

const origFetch = globalThis.fetch;

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  globalThis.fetch = origFetch;
  vi.restoreAllMocks();
});

describe('breakActivityToPane', () => {
  it.each([
    'horizontal',
    'vertical',
  ] as const)('issues POST .../activities/{aid}/break-to-pane with %s orientation', async (orientation) => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 201 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await breakActivityToPane('wid-1', 'pid-42', 'aid-7', orientation);
    expect(fetchMock).toHaveBeenCalledWith(
      '/windows/wid-1/panes/pid-42/activities/aid-7/break-to-pane',
      {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ orientation }),
      },
    );
  });

  it('warns on a 409 (single-activity pane) response', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 409 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await breakActivityToPane('wid-1', 'pid-1', 'aid-1', 'horizontal');
    expect(console.warn).toHaveBeenCalledWith(
      'break activity to pane failed',
      expect.objectContaining({
        windowId: 'wid-1',
        paneId: 'pid-1',
        activityId: 'aid-1',
        orientation: 'horizontal',
        status: 409,
      }),
    );
  });

  it('warns on a network failure', async () => {
    const err = new Error('net');
    const fetchMock = vi.fn().mockRejectedValue(err);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await breakActivityToPane('wid-1', 'pid-x', 'aid-x', 'vertical');
    expect(console.warn).toHaveBeenCalledWith(
      'break activity to pane request errored',
      expect.objectContaining({
        windowId: 'wid-1',
        paneId: 'pid-x',
        activityId: 'aid-x',
        orientation: 'vertical',
        error: err,
      }),
    );
  });
});
