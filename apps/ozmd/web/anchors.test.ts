import { describe, expect, it } from 'vitest';
import { installHeadingAnchors } from './anchors';

function build(html: string): HTMLElement {
  const div = document.createElement('div');
  div.innerHTML = html;
  installHeadingAnchors(div);
  return div;
}

describe('installHeadingAnchors', () => {
  it('injects a slug-id span into each heading', () => {
    const div = build('<h1 id="h0">Mounting</h1><h2 id="h1">Foo Bar</h2>');
    expect(div.querySelector('h1 > span.ozmd-anchor')?.id).toBe('mounting');
    expect(div.querySelector('h2 > span.ozmd-anchor')?.id).toBe('foo-bar');
  });

  it('preserves the existing h{n} heading ids', () => {
    const div = build('<h1 id="h0">Title</h1>');
    expect(div.querySelector('h1')?.id).toBe('h0');
  });

  it('dedupes colliding slugs', () => {
    const div = build('<h2>Repeat</h2><h2>Repeat</h2>');
    const ids = Array.from(div.querySelectorAll('span.ozmd-anchor')).map((s) => s.id);
    expect(ids).toEqual(['repeat', 'repeat-1']);
  });
});
