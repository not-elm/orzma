import { afterEach, describe, expect, it, vi } from 'vitest';
import { getBrowserConfig, loadBrowserConfig } from './browser';

const origFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = origFetch;
});

describe('loadBrowserConfig', () => {
  it('populates the singleton from /configs/browser', async () => {
    globalThis.fetch = vi.fn<typeof fetch>().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ search_template: 'https://www.google.com/search?q={query}' }),
    } as Response);
    await loadBrowserConfig();
    expect(getBrowserConfig().searchTemplate).toBe('https://www.google.com/search?q={query}');
  });

  it('falls back to the DuckDuckGo default when fetch fails', async () => {
    globalThis.fetch = vi
      .fn<typeof fetch>()
      .mockResolvedValue({ ok: false, status: 500, statusText: 'err' } as Response);
    await loadBrowserConfig();
    expect(getBrowserConfig().searchTemplate).toContain('duckduckgo.com');
  });

  it('falls back to the default when the server returns an empty template', async () => {
    globalThis.fetch = vi.fn<typeof fetch>().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ search_template: '' }),
    } as Response);
    await loadBrowserConfig();
    expect(getBrowserConfig().searchTemplate).toContain('duckduckgo.com');
  });
});
