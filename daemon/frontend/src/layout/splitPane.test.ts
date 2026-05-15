import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { splitPane } from './splitPane';

const origFetch = globalThis.fetch;

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  globalThis.fetch = origFetch;
  vi.restoreAllMocks();
});

describe('splitPane', () => {
  it.each([
    'horizontal',
    'vertical',
  ] as const)('issues POST /windows/{wid}/panes/{pid}/split with %s orientation', async (orientation) => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 201 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await splitPane('wid-1', 'pid-42', orientation);
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-42/split', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ orientation }),
    });
  });

  it('warns on 500 (PTY spawn failed → daemon rollback) with payload', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 500 } as Response);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await splitPane('wid-1', 'pid-1', 'horizontal');
    expect(console.warn).toHaveBeenCalledWith(
      'split pane failed',
      expect.objectContaining({
        windowId: 'wid-1',
        paneId: 'pid-1',
        orientation: 'horizontal',
        status: 500,
      }),
    );
  });

  it('warns on network failure with payload', async () => {
    const err = new Error('net');
    const fetchMock = vi.fn().mockRejectedValue(err);
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
    await splitPane('wid-1', 'pid-x', 'vertical');
    expect(console.warn).toHaveBeenCalledWith(
      'split pane request errored',
      expect.objectContaining({
        windowId: 'wid-1',
        paneId: 'pid-x',
        orientation: 'vertical',
        error: err,
      }),
    );
  });
});
