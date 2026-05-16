import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cycleActivity } from './cycleActivity';

const origFetch = globalThis.fetch;

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  globalThis.fetch = origFetch;
  vi.restoreAllMocks();
});

describe('cycleActivity', () => {
  it.each([
    'next',
    'prev',
  ] as const)('POSTs /windows/{wid}/panes/{pid}/cycle-activity with %s direction', async (direction) => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await cycleActivity('wid-1', 'pid-42', direction);
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-42/cycle-activity', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ direction }),
    });
  });

  it('warns on !ok status', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 404 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await cycleActivity('wid-1', 'pid-1', 'next');
    expect(console.warn).toHaveBeenCalledWith(
      'cycle activity failed',
      expect.objectContaining({
        windowId: 'wid-1',
        paneId: 'pid-1',
        direction: 'next',
        status: 404,
      }),
    );
  });

  it('warns on network failure', async () => {
    const err = new Error('net');
    const fetchMock = vi.fn().mockRejectedValue(err);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await cycleActivity('wid-1', 'pid-x', 'prev');
    expect(console.warn).toHaveBeenCalledWith(
      'cycle activity request errored',
      expect.objectContaining({
        windowId: 'wid-1',
        paneId: 'pid-x',
        direction: 'prev',
        error: err,
      }),
    );
  });
});
