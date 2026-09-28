/** Counts of an in-page search, and whether the last move wrapped around. */
export interface SearchResult {
  /** Total matches. */
  total: number;
  /** 1-based index of the current match (0 when none). */
  current: number;
  /** Whether the last move went past either end of the document. */
  wrapped: boolean;
}

/** Paints the match ranges. */
export interface HighlightPainter {
  /** Paints `all` as matches and `current` above them as the current match. */
  paint(all: readonly AbstractRange[], current: AbstractRange | null): void;
  /** Moves the current-match paint to `current`, leaving the other matches as painted. */
  paintCurrent(current: AbstractRange | null): void;
  /** Removes every painted match. */
  clear(): void;
}

/** Returns the top of `range` in document coordinates (CSS pixels from the page top). */
type MeasureTop = (range: AbstractRange) => number;

/** The vertical band of the viewport, in viewport coordinates, where a match counts as visible. */
interface VisibleBand {
  top: number;
  bottom: number;
}

const SYNTAX_CHARACTERS = /[\\^$.*+?()[\]{}|/]/g;

/** Whether smartcase makes `query` case-sensitive: it contains an uppercase letter. */
export function isCaseSensitive(query: string): boolean {
  return query !== query.toLowerCase();
}

/**
 * Returns a StaticRange for every literal occurrence of `query` in the text nodes under `root`,
 * in document order. Text inside SVG is skipped; a match never spans two text nodes.
 */
function findMatches(root: Node, query: string): StaticRange[] {
  if (query.length === 0) {
    return [];
  }
  const pattern = new RegExp(
    query.replace(SYNTAX_CHARACTERS, '\\$&'),
    isCaseSensitive(query) ? 'gu' : 'giu',
  );
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) =>
      node.parentElement?.closest('svg') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
  });
  const ranges: StaticRange[] = [];
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    for (const match of (node.nodeValue ?? '').matchAll(pattern)) {
      const start = match.index ?? 0;
      ranges.push(
        new StaticRange({
          startContainer: node,
          startOffset: start,
          endContainer: node,
          endOffset: start + match[0].length,
        }),
      );
    }
  }
  return ranges;
}

/** A live Range over the same boundary points as `range`. */
function toLiveRange(range: AbstractRange): Range {
  const live = document.createRange();
  live.setStart(range.startContainer, range.startOffset);
  live.setEnd(range.endContainer, range.endOffset);
  return live;
}

/** Measures `range` against the live layout. */
export function measureTop(range: AbstractRange): number {
  return toLiveRange(range).getBoundingClientRect().top + window.scrollY;
}

/** Whether a match spanning viewport y `top`..`bottom` lies outside `band`. */
function outsideBand(top: number, bottom: number, band: VisibleBand): boolean {
  return top < band.top || bottom > band.bottom;
}

/**
 * Scrolls `range` into view. The window scrolls, putting the match a third of the way down but
 * never above `band`, only when the match lies outside `band`; the nearest horizontally scrolled
 * ancestor scrolls when the match lies outside its box. A range that is not rendered, such as one
 * in a closed `<details>` or in a replaced document, scrolls nothing.
 */
export function revealRange(range: Range, band: VisibleBand): void {
  const rect = range.getBoundingClientRect();
  if (rect.width === 0 && rect.height === 0) {
    return;
  }
  if (outsideBand(rect.top, rect.bottom, band)) {
    const stop = Math.max(band.top, window.innerHeight / 3);
    window.scrollTo({ top: window.scrollY + rect.top - stop });
  }
  const scroller = horizontalScroller(range.startContainer.parentElement);
  if (scroller === null) {
    return;
  }
  const box = scroller.getBoundingClientRect();
  if (rect.left < box.left || rect.right > box.right) {
    scroller.scrollLeft += rect.left - box.left - box.width / 3;
  }
}

/** Paints matches with the CSS Custom Highlight API (`orzmd-match` and `orzmd-current`). */
export class CssHighlightPainter implements HighlightPainter {
  paint(all: readonly AbstractRange[], current: AbstractRange | null): void {
    const matches = new Highlight();
    for (const range of all) {
      matches.add(range);
    }
    CSS.highlights.set('orzmd-match', matches);
    this.paintCurrent(current);
  }

  paintCurrent(current: AbstractRange | null): void {
    const highlight = current === null ? new Highlight() : new Highlight(current);
    highlight.priority = 1;
    CSS.highlights.set('orzmd-current', highlight);
  }

  clear(): void {
    CSS.highlights.delete('orzmd-match');
    CSS.highlights.delete('orzmd-current');
  }
}

/** In-page text search that tracks the current match. */
export class Search {
  private readonly painter: HighlightPainter;
  private readonly measure: MeasureTop;
  private ranges: StaticRange[] = [];
  private index = 0;
  private query = '';

  constructor(painter: HighlightPainter, measure: MeasureTop) {
    this.painter = painter;
    this.measure = measure;
  }

  /**
   * Highlights `query` under `root`. The current match is the first one whose top is at or
   * below document y `originY`, or the first match when none is. Match tops are assumed not to
   * decrease in document order.
   */
  run(root: Node, query: string, originY: number): SearchResult {
    this.query = query;
    this.ranges = findMatches(root, query);
    let low = 0;
    let high = this.ranges.length;
    while (low < high) {
      const mid = (low + high) >> 1;
      if (this.measure(this.ranges[mid]) < originY) {
        low = mid + 1;
      } else {
        high = mid;
      }
    }
    this.index = low < this.ranges.length ? low : 0;
    this.paint();
    return this.result(false);
  }

  /** Searches `root` again for the same query, keeping the current number within the new total. */
  rerun(root: Node): SearchResult {
    this.ranges = findMatches(root, this.query);
    this.index = Math.min(this.index, Math.max(this.ranges.length - 1, 0));
    this.paint();
    return this.result(false);
  }

  /** Moves to the next or previous match, wrapping at either end. */
  navigate(dir: 'next' | 'prev'): SearchResult {
    const n = this.ranges.length;
    if (n === 0) {
      return this.result(false);
    }
    const next = dir === 'next' ? this.index + 1 : this.index - 1;
    this.index = (next + n) % n;
    this.painter.paintCurrent(this.ranges[this.index]);
    return this.result(next < 0 || next >= n);
  }

  /** Removes every highlight and forgets the query. */
  clear(): void {
    this.painter.clear();
    this.ranges = [];
    this.index = 0;
    this.query = '';
  }

  /** A live Range over the current match, or `null` when there is none. */
  currentRange(): Range | null {
    const current = this.ranges[this.index];
    return current === undefined ? null : toLiveRange(current);
  }

  private paint(): void {
    this.painter.paint(this.ranges, this.ranges[this.index] ?? null);
  }

  private result(wrapped: boolean): SearchResult {
    const total = this.ranges.length;
    return { total, current: total === 0 ? 0 : this.index + 1, wrapped };
  }
}

function horizontalScroller(from: Element | null): HTMLElement | null {
  for (let el = from; el !== null; el = el.parentElement) {
    if (
      el instanceof HTMLElement &&
      el.scrollWidth > el.clientWidth &&
      getComputedStyle(el).overflowX !== 'visible'
    ) {
      return el;
    }
  }
  return null;
}

/** Test-only exports of the file-local helpers (see typescript.md visibility rule). */
export const __testables = { findMatches, outsideBand };
