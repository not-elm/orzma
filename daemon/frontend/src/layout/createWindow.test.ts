import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createWindow } from './createWindow';

const originalFetch = globalThis.fetch;

describe('createWindow', () => {
  beforeEach(() => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it('POSTs /windows with the session id', async () => {
    const fetchSpy = vi.fn(async () => new Response(null, { status: 201 }));
    globalThis.fetch = fetchSpy as unknown as typeof fetch;
    await createWindow('sid-3');
    expect(fetchSpy).toHaveBeenCalledWith('/windows', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ session_id: 'sid-3' }),
    });
  });

  it('warns on non-ok response', async () => {
    globalThis.fetch = (async () => new Response(null, { status: 500 })) as unknown as typeof fetch;
    await createWindow('sid-3');
    expect(console.warn).toHaveBeenCalledWith(
      'window create failed',
      expect.objectContaining({ sid: 'sid-3', status: 500 }),
    );
  });

  it('warns on thrown error', async () => {
    globalThis.fetch = (async () => {
      throw new Error('boom');
    }) as unknown as typeof fetch;
    await createWindow('sid-3');
    expect(console.warn).toHaveBeenCalledWith(
      'window create request errored',
      expect.objectContaining({ sid: 'sid-3' }),
    );
  });
});
