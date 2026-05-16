import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { focusPane } from './focusPane';
import type { WindowId } from './types';

describe('focusPane', () => {
  const wid = 'w1' as WindowId;

  beforeEach(() => {
    vi.spyOn(global, 'fetch').mockResolvedValue(new Response(null, { status: 204 }));
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it.each([
    'up',
    'down',
    'left',
    'right',
  ] as const)('POSTs %s direction to the focus-pane endpoint', async (direction) => {
    await focusPane(wid, direction);
    expect(global.fetch).toHaveBeenCalledWith(`/windows/${wid}/focus-pane`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ direction }),
    });
  });

  it('console.warns on non-2xx response', async () => {
    (global.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      new Response(null, { status: 500 }),
    );
    await focusPane(wid, 'left');
    expect(console.warn).toHaveBeenCalled();
  });

  it('console.warns when fetch rejects', async () => {
    (global.fetch as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('net down'));
    await focusPane(wid, 'right');
    expect(console.warn).toHaveBeenCalled();
  });
});
