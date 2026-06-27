import { describe, expect, it } from 'vitest';
import { classifyLink } from './links';

describe('classifyLink', () => {
  it('treats empty and unsupported schemes as ignore', () => {
    expect(classifyLink('')).toEqual({ kind: 'ignore' });
    expect(classifyLink('javascript:alert(1)')).toEqual({ kind: 'ignore' });
    expect(classifyLink('data:text/html,x')).toEqual({ kind: 'ignore' });
    expect(classifyLink('file:///etc/passwd')).toEqual({ kind: 'ignore' });
  });

  it('classifies in-page anchors', () => {
    expect(classifyLink('#mounting')).toEqual({ kind: 'anchor', fragment: 'mounting' });
  });

  it('classifies external schemes', () => {
    expect(classifyLink('https://example.com')).toEqual({
      kind: 'external',
      url: 'https://example.com',
    });
    expect(classifyLink('http://example.com')).toEqual({
      kind: 'external',
      url: 'http://example.com',
    });
    expect(classifyLink('mailto:a@b.com')).toEqual({ kind: 'external', url: 'mailto:a@b.com' });
    expect(classifyLink('tel:+1')).toEqual({ kind: 'external', url: 'tel:+1' });
  });

  it('classifies markdown links with optional fragment', () => {
    expect(classifyLink('docs/osc.md')).toEqual({
      kind: 'markdown',
      path: 'docs/osc.md',
      fragment: null,
    });
    expect(classifyLink('../a.md#sec')).toEqual({
      kind: 'markdown',
      path: '../a.md',
      fragment: 'sec',
    });
    expect(classifyLink('/abs/x.MARKDOWN')).toEqual({
      kind: 'markdown',
      path: '/abs/x.MARKDOWN',
      fragment: null,
    });
  });

  it('classifies other local files', () => {
    expect(classifyLink('img.png')).toEqual({ kind: 'file', path: 'img.png' });
    expect(classifyLink('examples/foo.rs')).toEqual({ kind: 'file', path: 'examples/foo.rs' });
  });

  it('percent-decodes local paths', () => {
    expect(classifyLink('docs/a%20b.md')).toEqual({
      kind: 'markdown',
      path: 'docs/a b.md',
      fragment: null,
    });
  });

  it('falls back to the raw path on malformed percent-encoding', () => {
    expect(classifyLink('docs/a%ZZb.md')).toEqual({
      kind: 'markdown',
      path: 'docs/a%ZZb.md',
      fragment: null,
    });
  });
});
