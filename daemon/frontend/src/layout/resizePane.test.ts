import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { resizePane } from './resizePane';
import type { PaneId, WindowId } from './types';

describe('resizePane', () => {
  const wid = 'w1' as WindowId;
  const pid = 'p1' as PaneId;

  beforeEach(() => {
    vi.spyOn(global, 'fetch').mockResolvedValue(new Response(null, { status: 204 }));
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.spyOn(console, 'debug').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it.each([
    'left',
    'right',
    'up',
    'down',
  ] as const)('POSTs %s direction with default amount=1', async (direction) => {
    await resizePane(wid, pid, direction);
    expect(global.fetch).toHaveBeenCalledWith(`/windows/${wid}/panes/${pid}/resize`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ direction, amount: 1 }),
    });
  });

  it('forwards an explicit amount when provided', async () => {
    await resizePane(wid, pid, 'right', 5);
    expect(global.fetch).toHaveBeenCalledWith(`/windows/${wid}/panes/${pid}/resize`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ direction: 'right', amount: 5 }),
    });
  });

  it('logs at debug and returns without throwing on 409', async () => {
    (global.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      new Response('clamped at edge', { status: 409 }),
    );
    await expect(resizePane(wid, pid, 'left')).resolves.toBeUndefined();
    expect(console.debug).toHaveBeenCalled();
    expect(console.warn).not.toHaveBeenCalled();
  });

  it('console.warns on other non-2xx responses', async () => {
    (global.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      new Response(null, { status: 500 }),
    );
    await resizePane(wid, pid, 'left');
    expect(console.warn).toHaveBeenCalled();
  });

  it('console.warns when fetch rejects', async () => {
    (global.fetch as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('net down'));
    await resizePane(wid, pid, 'up');
    expect(console.warn).toHaveBeenCalled();
  });
});
