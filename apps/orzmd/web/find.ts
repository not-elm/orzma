import { SCROLL_OFFSET } from './layout';
import { isCaseSensitive, type Search, type SearchResult } from './search';

const WRAP_NOTICE_MS = 1500;
const NO_RESULT: SearchResult = { total: 0, current: 0, wrapped: false };

/** Stage of the in-page search: no box, a query being typed, or confirmed matches highlighted. */
export type SearchStage = 'closed' | 'typing' | 'active';

/** What the find box needs from the page besides the search. */
interface FindDeps {
  /** The element whose text is searched. */
  content: HTMLElement;
  /** Runs `task` once, before the next paint. */
  schedule: (task: () => void) => void;
  /** Brings `range` into view below the find box. */
  reveal: (range: Range) => void;
  /** The document y the page is scrolled to. */
  scrollY: () => number;
  /** Scrolls the page to document y `y`. */
  scrollTo: (y: number) => void;
}

/** The find box: its input, its count, and the life of a search from typing to closing. */
export class FindBox {
  private readonly root: HTMLElement;
  private readonly search: Search;
  private readonly deps: FindDeps;
  private readonly input: HTMLInputElement;
  private readonly caseMark: HTMLElement;
  private readonly wrapMark: HTMLElement;
  private readonly number: HTMLElement;
  private readonly navButtons: HTMLButtonElement[];
  private current: SearchStage = 'closed';
  private origin = 0;
  private savedScroll = 0;
  private pending = false;
  private last: SearchResult = NO_RESULT;
  private wrapTimer: ReturnType<typeof setTimeout> | undefined;

  constructor(root: HTMLElement, search: Search, deps: FindDeps) {
    this.root = root;
    this.search = search;
    this.deps = deps;
    this.input = root.querySelector('input') as HTMLInputElement;
    this.input.readOnly = true;
    this.caseMark = root.querySelector('.find-case') as HTMLElement;
    this.wrapMark = root.querySelector('.find-wrap') as HTMLElement;
    this.number = root.querySelector('.find-num') as HTMLElement;
    this.navButtons = Array.from(
      root.querySelectorAll<HTMLButtonElement>('[data-act="prev"], [data-act="next"]'),
    );
    this.input.addEventListener('input', (e) => {
      if (this.current === 'typing' && !(e as InputEvent).isComposing) {
        this.scheduleSearch();
      }
    });
    this.input.addEventListener('compositionend', () => {
      if (this.current === 'typing') {
        this.scheduleSearch();
      }
    });
    this.input.addEventListener('keydown', (e) => this.onKeydown(e));
    this.input.addEventListener('blur', () => this.resolve());
    for (const button of root.querySelectorAll('button')) {
      button.addEventListener('mousedown', (e) => e.preventDefault());
    }
    root.querySelector('[data-act="prev"]')?.addEventListener('click', () => this.nav('prev'));
    root.querySelector('[data-act="next"]')?.addEventListener('click', () => this.nav('next'));
    root.querySelector('[data-act="close"]')?.addEventListener('click', () => {
      if (this.current === 'typing') {
        this.escape();
      } else {
        this.clearHighlights();
      }
    });
  }

  /** The stage the box is in. */
  get stage(): SearchStage {
    return this.current;
  }

  /**
   * Starts typing a query: shows the box, remembers where the page is, focuses the input with its
   * text selected, and searches that text again. Does nothing while a query is already typed.
   */
  open(): void {
    if (this.current === 'typing') {
      return;
    }
    this.current = 'typing';
    this.root.hidden = false;
    this.input.readOnly = false;
    this.savedScroll = this.deps.scrollY();
    this.origin = this.savedScroll + SCROLL_OFFSET;
    this.input.focus();
    this.input.select();
    if (this.input.value.length > 0) {
      this.scheduleSearch();
    }
  }

  /** Types `text` into the input as a key press would, replacing the selection. Ignored outside typing. */
  typeText(text: string): void {
    if (this.current !== 'typing') {
      return;
    }
    const end = this.input.value.length;
    this.input.setRangeText(
      text,
      this.input.selectionStart ?? end,
      this.input.selectionEnd ?? end,
      'end',
    );
    this.scheduleSearch();
  }

  /** Deletes backwards in the input as Backspace would. Ignored outside typing. */
  backspace(): void {
    if (this.current !== 'typing') {
      return;
    }
    const start = this.input.selectionStart ?? this.input.value.length;
    const end = this.input.selectionEnd ?? start;
    if (start !== end) {
      this.input.setRangeText('', start, end, 'end');
    } else if (start > 0) {
      this.input.setRangeText('', start - 1, start, 'end');
    }
    this.scheduleSearch();
  }

  /** Handles Enter: confirms the search when there is at least one match. Ignored outside typing. */
  enter(): void {
    if (this.current !== 'typing') {
      return;
    }
    this.flush();
    if (this.last.total > 0) {
      this.finish('active');
    }
  }

  /** Handles Escape: closes the box and returns to where the search started. Ignored outside typing. */
  escape(): void {
    if (this.current !== 'typing') {
      return;
    }
    this.finish('closed');
    this.deps.scrollTo(this.savedScroll);
  }

  /** Moves to the next or previous match. Ignored while the box is closed. */
  nav(dir: 'next' | 'prev'): void {
    if (this.current === 'closed') {
      return;
    }
    this.render(this.search.navigate(dir));
    this.revealCurrent();
  }

  /** Closes a confirmed search and removes its highlights. Does nothing in any other stage. */
  clearHighlights(): void {
    if (this.current === 'active') {
      this.finish('closed');
    }
  }

  /** Closes the box from any stage without moving the page, as when another document replaces this one. */
  reset(): void {
    if (this.current !== 'closed') {
      this.finish('closed');
    }
  }

  /** Searches the re-rendered document again while a search is open. */
  rerun(): void {
    if (this.current !== 'closed' && this.input.value.length > 0) {
      this.render(this.search.rerun(this.deps.content));
    }
  }

  private onKeydown(e: KeyboardEvent): void {
    if (e.isComposing || e.keyCode === 229) {
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      this.enter();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      this.escape();
    } else if (e.key === 'Tab') {
      e.preventDefault();
    }
  }

  /** Ends a typed search when the input loses focus: confirmed with a match, closed without, and the page stays where it is. */
  private resolve(): void {
    if (this.current !== 'typing') {
      return;
    }
    this.flush();
    this.finish(this.last.total > 0 ? 'active' : 'closed');
  }

  /**
   * Leaves the current stage for `next`: the input turns read-only and loses focus, and closing
   * hides the box and removes the highlights. The stage changes before the input loses focus, so
   * the input's own blur does nothing.
   */
  private finish(next: 'active' | 'closed'): void {
    this.current = next;
    this.input.readOnly = true;
    this.root.hidden = next === 'closed';
    if (next === 'closed') {
      this.clearMatches();
    }
    this.input.blur();
  }

  private clearMatches(): void {
    this.pending = false;
    this.search.clear();
    this.render(null);
  }

  private scheduleSearch(): void {
    if (this.pending) {
      return;
    }
    this.pending = true;
    this.deps.schedule(() => this.flush());
  }

  private flush(): void {
    if (!this.pending) {
      return;
    }
    this.pending = false;
    this.render(this.search.run(this.deps.content, this.input.value, this.origin));
    this.revealCurrent();
  }

  private revealCurrent(): void {
    const range = this.search.currentRange();
    if (range !== null) {
      this.deps.reveal(range);
    }
  }

  /** Shows `result`; `null` stands for cleared matches and shows no count. */
  private render(result: SearchResult | null): void {
    this.last = result ?? NO_RESULT;
    const query = this.input.value;
    const none = result !== null && query.length > 0 && result.total === 0;
    this.caseMark.classList.toggle('on', isCaseSensitive(query));
    this.root.classList.toggle('no-results', none);
    for (const button of this.navButtons) {
      button.disabled = this.last.total === 0;
    }
    if (result === null || query.length === 0) {
      this.number.textContent = '';
    } else if (none) {
      this.number.textContent = 'No results';
    } else {
      this.number.textContent = `${result.current} / ${result.total}`;
    }
    clearTimeout(this.wrapTimer);
    this.wrapMark.hidden = !result?.wrapped;
    if (result?.wrapped) {
      this.wrapTimer = setTimeout(() => {
        this.wrapMark.hidden = true;
      }, WRAP_NOTICE_MS);
    }
  }
}
