import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { windowSelect } from './windowSelect';

const originalFetch = globalThis.fetch;

describe('windowSelect', () => {
  beforeEach(() => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it('POSTs to /windows/{wid}/select', async () => {
    const fetchSpy = vi.fn(async () => new Response(null, { status: 204 }));
    globalThis.fetch = fetchSpy as unknown as typeof fetch;
    await windowSelect('wid-7');
    expect(fetchSpy).toHaveBeenCalledWith('/windows/wid-7/select', { method: 'POST' });
  });

  it('warns on non-ok response', async () => {
    globalThis.fetch = (async () => new Response(null, { status: 500 })) as unknown as typeof fetch;
    await windowSelect('wid-7');
    expect(console.warn).toHaveBeenCalledWith(
      'window select failed',
      expect.objectContaining({ wid: 'wid-7', status: 500 }),
    );
  });

  it('warns on thrown error', async () => {
    globalThis.fetch = (async () => {
      throw new Error('boom');
    }) as unknown as typeof fetch;
    await windowSelect('wid-7');
    expect(console.warn).toHaveBeenCalledWith(
      'window select request errored',
      expect.objectContaining({ wid: 'wid-7' }),
    );
  });
});
