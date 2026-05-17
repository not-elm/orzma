import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { renameWindow } from './renameWindow';

const originalFetch = globalThis.fetch;

describe('renameWindow', () => {
  beforeEach(() => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it('PATCHes /windows/{wid} with the new name', async () => {
    const fetchSpy = vi.fn(async () => new Response(null, { status: 204 }));
    globalThis.fetch = fetchSpy as unknown as typeof fetch;
    await renameWindow('wid-7', 'new-name');
    expect(fetchSpy).toHaveBeenCalledWith('/windows/wid-7', {
      method: 'PATCH',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ name: 'new-name' }),
    });
  });

  it('warns on non-ok response', async () => {
    globalThis.fetch = (async () => new Response(null, { status: 500 })) as unknown as typeof fetch;
    await renameWindow('wid-7', 'x');
    expect(console.warn).toHaveBeenCalledWith(
      'window rename failed',
      expect.objectContaining({ wid: 'wid-7', status: 500 }),
    );
  });

  it('warns on thrown error', async () => {
    globalThis.fetch = (async () => {
      throw new Error('boom');
    }) as unknown as typeof fetch;
    await renameWindow('wid-7', 'x');
    expect(console.warn).toHaveBeenCalledWith(
      'window rename request errored',
      expect.objectContaining({ wid: 'wid-7' }),
    );
  });
});
