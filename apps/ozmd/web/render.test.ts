import { describe, expect, it } from 'vitest';
import { renderMarkdown } from './render';

describe('renderMarkdown', () => {
  it('tags each markdown heading with a sequential id in document order', () => {
    const html = renderMarkdown('# A\n\n## B\n\n### C\n');
    const doc = new DOMParser().parseFromString(html, 'text/html');
    const ids = Array.from(doc.querySelectorAll('h1,h2,h3')).map((h) => h.id);
    expect(ids).toEqual(['h0', 'h1', 'h2']);
  });

  it('does not tag raw-html headings (index alignment with the Rust outline)', () => {
    const html = renderMarkdown('# Real\n\n<h2>Raw</h2>\n\n## Second\n');
    const doc = new DOMParser().parseFromString(html, 'text/html');
    const tagged = Array.from(doc.querySelectorAll('h1,h2,h3,h4,h5,h6'))
      .map((h) => h.id)
      .filter((id) => /^h\d+$/.test(id));
    expect(tagged).toEqual(['h0', 'h1']);
  });

  it('adds highlight.js classes to fenced code', () => {
    const html = renderMarkdown('```js\nconst x = 1;\n```\n');
    expect(html).toContain('hljs');
  });

  it('strips script tags from untrusted html', () => {
    const html = renderMarkdown('hello <script>alert(1)</script> world');
    expect(html).not.toContain('<script>');
  });

  it('strips event-handler attributes', () => {
    expect(renderMarkdown('<img src="x" onerror="alert(1)">')).not.toContain('onerror');
  });

  it('strips javascript: urls in links', () => {
    expect(renderMarkdown('[x](javascript:alert(1))')).not.toContain('javascript:');
  });

  it('resets heading ids between calls', () => {
    renderMarkdown('# first\n');
    const html = renderMarkdown('# again\n');
    const doc = new DOMParser().parseFromString(html, 'text/html');
    expect(doc.querySelector('h1')?.id).toBe('h0');
  });
});
