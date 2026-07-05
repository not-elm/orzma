import { describe, expect, it } from 'vitest';
import { collectLocalImages, isLocalImageSrc, toLocalPath } from './images';

describe('isLocalImageSrc', () => {
  it('accepts relative and absolute filesystem paths', () => {
    expect(isLocalImageSrc('img/a.png')).toBe(true);
    expect(isLocalImageSrc('/Users/me/a.png')).toBe(true);
    expect(isLocalImageSrc('../shared/a.png')).toBe(true);
  });

  it('rejects remote, data, blob, file, protocol-relative, anchors, and empty', () => {
    expect(isLocalImageSrc('https://x/a.png')).toBe(false);
    expect(isLocalImageSrc('http://x/a.png')).toBe(false);
    expect(isLocalImageSrc('data:image/png;base64,AAAA')).toBe(false);
    expect(isLocalImageSrc('blob:abc')).toBe(false);
    expect(isLocalImageSrc('file:///a.png')).toBe(false);
    expect(isLocalImageSrc('//cdn/a.png')).toBe(false);
    expect(isLocalImageSrc('#frag')).toBe(false);
    expect(isLocalImageSrc('')).toBe(false);
  });
});

describe('toLocalPath', () => {
  it('strips query and fragment and percent-decodes', () => {
    expect(toLocalPath('img/a%20b.png?v=2#x')).toBe('img/a b.png');
    expect(toLocalPath('/abs/p.png')).toBe('/abs/p.png');
  });
});

describe('collectLocalImages', () => {
  it('collects local imgs with decoded paths and skips remote', () => {
    document.body.innerHTML =
      '<img src="img/a.png"><img src="https://x/b.png"><img src="/c%20d.png?z">';
    const got = collectLocalImages(document.body);
    expect(got.map((g) => g.path)).toEqual(['img/a.png', '/c d.png']);
    expect(got.every((g) => g.el instanceof HTMLImageElement)).toBe(true);
  });
});
