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

  it('returns true on 200 OK', async () => {
    const fetchSpy = vi.fn(async () => new Response(null, { status: 200 }));
    globalThis.fetch = fetchSpy as unknown as typeof fetch;
    const result = await windowSelect('wid-7');
    expect(result).toBe(true);
    expect(fetchSpy).toHaveBeenCalledWith('/windows/wid-7/select', { method: 'POST' });
  });

  it('returns true on 204 No Content', async () => {
    const fetchSpy = vi.fn(async () => new Response(null, { status: 204 }));
    globalThis.fetch = fetchSpy as unknown as typeof fetch;
    const result = await windowSelect('wid-7');
    expect(result).toBe(true);
  });

  it('returns false on non-ok response and warns', async () => {
    globalThis.fetch = (async () => new Response(null, { status: 500 })) as unknown as typeof fetch;
    const result = await windowSelect('wid-7');
    expect(result).toBe(false);
    expect(console.warn).toHaveBeenCalledWith(
      'window select failed',
      expect.objectContaining({ wid: 'wid-7', status: 500 }),
    );
  });

  it('returns false on thrown error and warns', async () => {
    globalThis.fetch = (async () => {
      throw new Error('boom');
    }) as unknown as typeof fetch;
    const result = await windowSelect('wid-7');
    expect(result).toBe(false);
    expect(console.warn).toHaveBeenCalledWith(
      'window select request errored',
      expect.objectContaining({ wid: 'wid-7' }),
    );
  });
});
