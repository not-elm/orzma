/** Result counts of an in-page search. */
export interface SearchResult {
  /** Total matches. */
  total: number;
  /** 1-based index of the current match (0 when none). */
  current: number;
}

/**
 * In-page text search: highlights matches, tracks the current one, scrolls to it.
 *
 * All `run` / `navigate` / `clear` calls must operate on the SAME root element;
 * the instance does not track which root its marks belong to, so switching roots
 * would leave stale highlights in the previous one.
 */
export class Search {
  private marks: HTMLElement[] = [];
  private index = 0;

  /** Highlights every case-insensitive occurrence of `query` under `root`. */
  run(root: HTMLElement, query: string): SearchResult {
    this.clear(root);
    if (query.length === 0) {
      return { total: 0, current: 0 };
    }
    const needle = query.toLowerCase();
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    const textNodes: Text[] = [];
    for (let n = walker.nextNode(); n !== null; n = walker.nextNode()) {
      textNodes.push(n as Text);
    }
    for (const node of textNodes) {
      this.markNode(node, needle);
    }
    this.index = 0;
    this.focusCurrent();
    return { total: this.marks.length, current: this.marks.length === 0 ? 0 : 1 };
  }

  /** Moves to the next/previous match (wrapping) and scrolls it into view. */
  navigate(dir: 'next' | 'prev'): SearchResult {
    if (this.marks.length === 0) {
      return { total: 0, current: 0 };
    }
    const n = this.marks.length;
    this.index = dir === 'next' ? (this.index + 1) % n : (this.index - 1 + n) % n;
    this.focusCurrent();
    return { total: n, current: this.index + 1 };
  }

  /** Removes all highlight marks under `root`. */
  clear(root: HTMLElement): void {
    for (const mark of root.querySelectorAll('mark.ozmd-match')) {
      const text = document.createTextNode(mark.textContent ?? '');
      mark.replaceWith(text);
    }
    root.normalize();
    this.marks = [];
    this.index = 0;
  }

  private markNode(node: Text, needle: string): void {
    const value = node.nodeValue ?? '';
    const lower = value.toLowerCase();
    let from = lower.indexOf(needle);
    if (from === -1) {
      return;
    }
    const frag = document.createDocumentFragment();
    let cursor = 0;
    while (from !== -1) {
      if (from > cursor) {
        frag.append(document.createTextNode(value.slice(cursor, from)));
      }
      const mark = document.createElement('mark');
      mark.className = 'ozmd-match';
      mark.textContent = value.slice(from, from + needle.length);
      frag.append(mark);
      this.marks.push(mark);
      cursor = from + needle.length;
      from = lower.indexOf(needle, cursor);
    }
    if (cursor < value.length) {
      frag.append(document.createTextNode(value.slice(cursor)));
    }
    node.replaceWith(frag);
  }

  private focusCurrent(): void {
    this.marks.forEach((m, i) => m.classList.toggle('ozmd-current', i === this.index));
    const current = this.marks[this.index];
    if (current?.scrollIntoView) {
      current.scrollIntoView({ block: 'center' });
    }
  }
}
