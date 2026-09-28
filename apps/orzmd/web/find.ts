import type { SearchStage } from './chrome';
import { SCROLL_OFFSET } from './layout';
import { isCaseSensitive, type Search, type SearchResult } from './search';

const WRAP_NOTICE_MS = 1500;
const NO_RESULT: SearchResult = { total: 0, current: 0, wrapped: false };

/** Why the page ended a typed search. */
export type SearchCause = 'key' | 'blur';

/** Receives the end of a typed search. */
interface FindHost {
  /** Reports that the typed search was confirmed. */
  submit(cause: SearchCause): void;
  /** Reports that the typed search was abandoned. */
  escape(cause: SearchCause): void;
  /** Reports that the confirmed search was closed with the close button. */
  close(): void;
}

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

/**
 * The stage the find box is in: the controller's stages, plus `reported` for a typed search whose
 * end was reported before the controller's next stage arrived.
 */
type FindStage = SearchStage | 'reported';

/** The find box: its input, its count, and the life of a typed search. */
export class FindBox {
  private readonly root: HTMLElement;
  private readonly search: Search;
  private readonly host: FindHost;
  private readonly deps: FindDeps;
  private readonly input: HTMLInputElement;
  private readonly caseMark: HTMLElement;
  private readonly wrapMark: HTMLElement;
  private readonly number: HTMLElement;
  private readonly navButtons: HTMLButtonElement[];
  private stage: FindStage = 'closed';
  private origin = 0;
  private savedScroll = 0;
  private pending = false;
  private last: SearchResult = NO_RESULT;
  private wrapTimer: ReturnType<typeof setTimeout> | undefined;

  constructor(root: HTMLElement, search: Search, host: FindHost, deps: FindDeps) {
    this.root = root;
    this.search = search;
    this.host = host;
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
      if (this.stage === 'typing' && !(e as InputEvent).isComposing) {
        this.scheduleSearch();
      }
    });
    this.input.addEventListener('compositionend', () => {
      if (this.stage === 'typing') {
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
      if (this.stage === 'active') {
        this.host.close();
      } else {
        this.escape();
      }
    });
  }

  /**
   * Applies the stage the controller reports. Entering `typing` remembers the scroll position,
   * focuses the input with its text selected, and searches that text again; leaving to `closed`
   * removes the highlights. Outside `typing`, and once the typed search has reported its end, the
   * input is read-only; a `typing` stage arriving after that report changes nothing.
   */
  setStage(next: SearchStage): void {
    const previous = this.stage;
    if (next === previous || (next === 'typing' && previous === 'reported')) {
      return;
    }
    this.stage = next;
    this.root.hidden = next === 'closed';
    this.input.readOnly = next !== 'typing';
    if (next === 'typing') {
      this.savedScroll = this.deps.scrollY();
      this.origin = this.savedScroll + SCROLL_OFFSET;
      this.input.focus();
      this.input.select();
      if (this.input.value.length > 0) {
        this.scheduleSearch();
      }
    } else if (previous === 'typing' || previous === 'reported') {
      this.input.blur();
    }
    if (next === 'closed') {
      this.reset();
    }
  }

  /**
   * Types `text` into the input as a key press would, replacing the selection. Ignored outside a
   * typed search and once it has reported its end.
   */
  typeText(text: string): void {
    if (this.stage !== 'typing') {
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

  /**
   * Deletes backwards in the input as Backspace would. Ignored outside a typed search and once it
   * has reported its end.
   */
  backspace(): void {
    if (this.stage !== 'typing') {
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

  /**
   * Handles Enter: reports a confirmed search when there is at least one match. Does nothing
   * outside a typed search and once it has reported its end.
   */
  enter(): void {
    if (this.stage !== 'typing') {
      return;
    }
    this.flush();
    if (this.last.total > 0) {
      this.end('submit', 'key');
    }
  }

  /**
   * Ends a typed search by its match count: confirmed with a match, abandoned without. Does
   * nothing outside a typed search and once it has reported its end.
   */
  resolve(): void {
    if (this.stage !== 'typing') {
      return;
    }
    this.flush();
    this.end(this.last.total > 0 ? 'submit' : 'escape', 'blur');
  }

  /** Removes the highlights and returns to the position the search started from. */
  cancel(): void {
    this.reset();
    this.deps.scrollTo(this.savedScroll);
  }

  /** Moves to the next or previous match. */
  nav(dir: 'next' | 'prev'): void {
    this.render(this.search.navigate(dir));
    this.revealCurrent();
  }

  /** Searches the re-rendered document again while a search is open. */
  rerun(): void {
    if (this.stage !== 'closed' && this.input.value.length > 0) {
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

  private escape(): void {
    if (this.stage === 'typing') {
      this.end('escape', 'key');
    }
  }

  /** Reports the end of the typed search and stops taking input until the next stage. */
  private end(outcome: 'submit' | 'escape', cause: SearchCause): void {
    this.stage = 'reported';
    this.input.readOnly = true;
    if (outcome === 'submit') {
      this.host.submit(cause);
    } else {
      this.host.escape(cause);
    }
  }

  private reset(): void {
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
