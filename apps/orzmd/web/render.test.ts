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

  it('renders GFM tables', () => {
    const html = renderMarkdown('| a | b |\n| - | - |\n| 1 | 2 |\n');
    const doc = new DOMParser().parseFromString(html, 'text/html');
    expect(doc.querySelector('table')).not.toBeNull();
    expect(doc.querySelectorAll('td')).toHaveLength(2);
  });

  it('renders GFM task-list checkboxes', () => {
    const html = renderMarkdown('- [ ] todo\n- [x] done\n');
    const doc = new DOMParser().parseFromString(html, 'text/html');
    const boxes = doc.querySelectorAll('input[type="checkbox"]');
    expect(boxes).toHaveLength(2);
    expect(boxes[0].hasAttribute('checked')).toBe(false);
    expect(boxes[1].hasAttribute('checked')).toBe(true);
  });

  it('renders GFM strikethrough', () => {
    const html = renderMarkdown('~~struck~~\n');
    const doc = new DOMParser().parseFromString(html, 'text/html');
    expect(doc.querySelector('del')).not.toBeNull();
  });

  it.each([
    'note',
    'tip',
    'important',
    'warning',
    'caution',
  ])('renders a [!%s] blockquote as a styled alert callout', (type) => {
    const html = renderMarkdown(`> [!${type.toUpperCase()}]\n> body text\n`);
    const doc = new DOMParser().parseFromString(html, 'text/html');
    const alert = doc.querySelector(`div.markdown-alert.markdown-alert-${type}`);
    expect(alert).not.toBeNull();
    expect(alert?.querySelector('.markdown-alert-title')).not.toBeNull();
  });

  it('leaves a plain blockquote unstyled', () => {
    const html = renderMarkdown('> just a quote\n');
    const doc = new DOMParser().parseFromString(html, 'text/html');
    expect(doc.querySelector('blockquote')).not.toBeNull();
    expect(doc.querySelector('.markdown-alert')).toBeNull();
  });

  it('keeps the octicon svg in the alert title after sanitization', () => {
    const html = renderMarkdown('> [!NOTE]\n> body\n');
    const doc = new DOMParser().parseFromString(html, 'text/html');
    expect(doc.querySelector('.markdown-alert-title svg')).not.toBeNull();
  });

  it('numbers headings inside and outside an alert in document order', () => {
    const html = renderMarkdown('> [!NOTE]\n> # Inside alert\n\n# Top level\n');
    const doc = new DOMParser().parseFromString(html, 'text/html');
    const inside = doc.querySelector('.markdown-alert h1');
    expect(inside).not.toBeNull();
    expect(inside?.id).toBe('h0');
    const ids = Array.from(doc.querySelectorAll('h1')).map((h) => h.id);
    expect(ids).toEqual(['h0', 'h1']);
  });
});
