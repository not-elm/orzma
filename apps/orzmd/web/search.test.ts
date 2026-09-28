import { describe, expect, it } from 'vitest';
import { __testables, type HighlightPainter, isCaseSensitive, Search } from './search';

const { findMatches, outsideBand } = __testables;

class RecordingPainter implements HighlightPainter {
  all: readonly AbstractRange[] = [];
  current: AbstractRange | null = null;
  cleared = 0;
  fullPaints = 0;
  paint(all: readonly AbstractRange[], current: AbstractRange | null): void {
    this.fullPaints++;
    this.all = all;
    this.current = current;
  }
  paintCurrent(current: AbstractRange | null): void {
    this.current = current;
  }
  clear(): void {
    this.cleared++;
    this.all = [];
    this.current = null;
  }
}

function container(html: string): HTMLElement {
  const el = document.createElement('div');
  el.innerHTML = html;
  document.body.replaceChildren(el);
  return el;
}

function measureByDataY(range: AbstractRange): number {
  return Number((range.startContainer.parentElement as HTMLElement).dataset.y ?? 0);
}

function text(range: AbstractRange): string {
  return (range.startContainer.nodeValue ?? '').slice(range.startOffset, range.endOffset);
}

function newSearch(): { search: Search; painter: RecordingPainter } {
  const painter = new RecordingPainter();
  return { search: new Search(painter, measureByDataY), painter };
}

describe('isCaseSensitive', () => {
  it('turns on only for a query with an uppercase letter', () => {
    expect(isCaseSensitive('abc')).toBe(false);
    expect(isCaseSensitive('aBc')).toBe(true);
    expect(isCaseSensitive('検索')).toBe(false);
  });
});

describe('findMatches', () => {
  it('ignores case for a lowercase query', () => {
    expect(findMatches(container('<p>foo Foo FOO bar</p>'), 'foo')).toHaveLength(3);
  });

  it('respects case once the query has an uppercase letter', () => {
    const ranges = findMatches(container('<p>foo Foo FOO</p>'), 'Foo');
    expect(ranges.map(text)).toEqual(['Foo']);
  });

  it('matches regex metacharacters literally', () => {
    const root = container('<p>a.b axb a+b (c) [d] \\e /f</p>');
    for (const q of ['a.b', 'a+b', '(c)', '[d]', '\\e', '/f']) {
      expect(findMatches(root, q).map(text)).toEqual([q]);
    }
  });

  it('places ranges on the original text when lowercasing changes its length', () => {
    const [range] = findMatches(container('<p>İx foo</p>'), 'foo');
    expect(range.startOffset).toBe(3);
    expect(text(range)).toBe('foo');
  });

  it('finds Japanese text', () => {
    expect(findMatches(container('<p>検索と検索</p>'), '検索')).toHaveLength(2);
  });

  it('skips text inside SVG', () => {
    expect(findMatches(container('<p>cat</p><svg><text>cat</text></svg>'), 'cat')).toHaveLength(1);
  });

  it('finds nothing for an empty query', () => {
    expect(findMatches(container('<p>anything</p>'), '')).toHaveLength(0);
  });

  it('counts every match in a large document', () => {
    const root = container('<p>needle hay</p>'.repeat(5000));
    expect(findMatches(root, 'needle')).toHaveLength(5000);
  });
});

describe('Search', () => {
  const three = '<p data-y="0">x</p><p data-y="100">x</p><p data-y="200">x</p>';

  it('starts at the first match at or below the origin', () => {
    const { search, painter } = newSearch();
    const root = container(three);
    expect(search.run(root, 'x', 150)).toEqual({ total: 3, current: 3, wrapped: false });
    expect(painter.all).toHaveLength(3);
    expect(painter.current?.startContainer.parentElement?.dataset.y).toBe('200');
  });

  it('starts at the first match when none is below the origin', () => {
    const { search } = newSearch();
    expect(search.run(container(three), 'x', 250).current).toBe(1);
  });

  it('reports a wrap when moving past either end', () => {
    const { search } = newSearch();
    search.run(container(three), 'x', 150);
    expect(search.navigate('next')).toEqual({ total: 3, current: 1, wrapped: true });
    expect(search.navigate('next')).toEqual({ total: 3, current: 2, wrapped: false });
    expect(search.navigate('prev')).toEqual({ total: 3, current: 1, wrapped: false });
    expect(search.navigate('prev')).toEqual({ total: 3, current: 3, wrapped: true });
  });

  it('repaints only the current match when moving', () => {
    const { search, painter } = newSearch();
    search.run(container(three), 'x', 0);
    search.navigate('next');
    expect(painter.fullPaints).toBe(1);
    expect(painter.current?.startContainer.parentElement?.dataset.y).toBe('100');
  });

  it('keeps the current number within a smaller total after a re-render', () => {
    const { search } = newSearch();
    const root = container(three);
    search.run(root, 'x', 150);
    root.innerHTML = '<p data-y="0">x</p><p data-y="100">x</p>';
    expect(search.rerun(root)).toEqual({ total: 2, current: 2, wrapped: false });
  });

  it('clears the painter and forgets the query', () => {
    const { search, painter } = newSearch();
    const root = container(three);
    search.run(root, 'x', 0);
    search.clear();
    expect(painter.cleared).toBe(1);
    expect(search.currentRange()).toBeNull();
    expect(search.rerun(root).total).toBe(0);
  });
});

describe('outsideBand', () => {
  it('flags a match above or below the visible band', () => {
    const band = { top: 38, bottom: 600 };
    expect(outsideBand(10, 30, band)).toBe(true);
    expect(outsideBand(590, 610, band)).toBe(true);
    expect(outsideBand(100, 120, band)).toBe(false);
  });
});
