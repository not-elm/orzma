import { afterEach, describe, expect, it, vi } from 'vitest';
import { getFontConfig, loadFontConfig, pointsToPx, preloadFonts } from './font';

const origFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = origFetch;
});

describe('loadFontConfig', () => {
  it('populates the singleton from /configs/font', async () => {
    globalThis.fetch = vi.fn<typeof fetch>().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({
        size: 18,
        normal_family: 'Hack Nerd Font',
        bold_family: 'Hack Nerd Font',
        italic_family: 'Hack Nerd Font',
        bold_italic_family: 'Hack Nerd Font',
      }),
    } as Response);
    await loadFontConfig();
    expect(getFontConfig()).toEqual({
      size: 18,
      normalFamily: 'Hack Nerd Font',
      boldFamily: 'Hack Nerd Font',
      italicFamily: 'Hack Nerd Font',
      boldItalicFamily: 'Hack Nerd Font',
    });
  });

  it('falls back to defaults when the fetch fails', async () => {
    globalThis.fetch = vi
      .fn<typeof fetch>()
      .mockResolvedValue({ ok: false, status: 500, statusText: 'err' } as Response);
    await loadFontConfig();
    expect(getFontConfig().size).toBe(11.25);
    expect(getFontConfig().normalFamily).toBe('monospace');
  });
});

describe('pointsToPx', () => {
  it("converts points to CSS pixels (Alacritty's 11.25pt = 15px)", () => {
    expect(pointsToPx(11.25)).toBe(15);
  });
});

describe('preloadFonts', () => {
  it('resolves without throwing when document.fonts is unavailable', async () => {
    const orig = document.fonts;
    Object.defineProperty(document, 'fonts', { value: undefined, configurable: true });
    try {
      await expect(preloadFonts()).resolves.toBeUndefined();
    } finally {
      Object.defineProperty(document, 'fonts', { value: orig, configurable: true });
    }
  });
});
