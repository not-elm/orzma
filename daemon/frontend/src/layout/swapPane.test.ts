import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { swapPane } from './swapPane';
import type { PaneId, WindowId } from './types';

describe('swapPane', () => {
  const wid = 'w1' as WindowId;
  const pid = 'p1' as PaneId;

  beforeEach(() => {
    vi.spyOn(global, 'fetch').mockResolvedValue(new Response(null, { status: 204 }));
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it.each([
    'prev',
    'next',
  ] as const)('POSTs %s offset to the swap-pane endpoint', async (offset) => {
    await swapPane(wid, pid, offset);
    expect(global.fetch).toHaveBeenCalledWith(`/windows/${wid}/panes/${pid}/swap`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ offset }),
    });
  });

  it('console.warns on non-2xx response', async () => {
    (global.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      new Response(null, { status: 500 }),
    );
    await swapPane(wid, pid, 'next');
    expect(console.warn).toHaveBeenCalled();
  });

  it('console.warns when fetch rejects', async () => {
    (global.fetch as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('net down'));
    await swapPane(wid, pid, 'prev');
    expect(console.warn).toHaveBeenCalled();
  });
});
