import { type HeadingInfo, headingLabel } from './chrome';

const INDENT_PX = 12;

/** What the outline panel asks of the page. */
interface OutlineHost {
  /** Scrolls heading `index` into view. */
  jump(index: number): void;
  /** Lays the document out for the panel being shown (`true`) or hidden, keeping the reading position. */
  relayout(open: boolean): void;
}

/** The outline sidebar: one entry per heading, the keyboard selection, and the section in view. */
export class OutlinePanel {
  private readonly root: HTMLElement;
  private readonly list: HTMLElement;
  private readonly host: OutlineHost;
  private entries: HTMLElement[] = [];
  private selected = 0;
  private current: number | null = null;
  private markedSelected = -1;
  private markedCurrent = -1;
  private revealPending = false;

  constructor(root: HTMLElement, host: OutlineHost) {
    this.root = root;
    this.list = root.querySelector('.outline-list') as HTMLElement;
    this.host = host;
    this.list.addEventListener('click', (e) => {
      const target = e.target instanceof Element ? e.target : null;
      const item = target?.closest<HTMLElement>('[data-index]');
      if (item?.dataset.index === undefined) {
        return;
      }
      this.selected = Number(item.dataset.index);
      this.render();
      this.host.jump(this.selected);
    });
  }

  /** Whether the panel is shown. */
  isOpen(): boolean {
    return !this.root.hidden;
  }

  /**
   * Shows the panel with heading `current` selected — the first heading for `null`, the last one
   * past the end — and scrolls the selection into view. Does nothing while the panel is shown.
   */
  open(current: number | null): void {
    if (this.isOpen()) {
      return;
    }
    this.selected = this.clamp(current ?? 0);
    this.root.hidden = false;
    this.revealPending = true;
    this.host.relayout(true);
    this.render();
  }

  /** Hides the panel. Does nothing while it is hidden. */
  close(): void {
    if (!this.isOpen()) {
      return;
    }
    this.root.hidden = true;
    this.host.relayout(false);
  }

  /** Moves the selection one heading down (`1`) or up (`-1`), stopping at either end. */
  move(delta: 1 | -1): void {
    if (this.entries.length === 0) {
      return;
    }
    this.selected = this.clamp(this.selected + delta);
    this.render();
  }

  /** Jumps to the selected heading. Does nothing without headings. */
  choose(): void {
    if (this.entries.length > 0) {
      this.host.jump(this.selected);
    }
  }

  /** Replaces the entries with `items`, indented relative to the shallowest heading, keeping the selection within them. */
  setItems(items: readonly HeadingInfo[]): void {
    this.markedSelected = -1;
    this.markedCurrent = -1;
    if (items.length === 0) {
      const empty = document.createElement('li');
      empty.className = 'outline-empty';
      empty.textContent = 'No headings';
      this.entries = [];
      this.selected = 0;
      this.list.replaceChildren(empty);
      return;
    }
    const shallowest = Math.min(...items.map((h) => h.level));
    this.entries = items.map((heading, index) => {
      const li = document.createElement('li');
      li.dataset.index = String(index);
      li.style.paddingLeft = `${INDENT_PX * (1 + heading.level - shallowest)}px`;
      li.textContent = headingLabel(heading.text);
      return li;
    });
    this.list.replaceChildren(...this.entries);
    this.selected = this.clamp(this.selected);
    this.render();
  }

  /** Marks heading `current` as the section in view, or none for `null`. Nothing is marked while the panel is hidden. */
  markCurrent(current: number | null): void {
    this.current = current;
    this.render();
  }

  private clamp(index: number): number {
    return Math.min(Math.max(index, 0), Math.max(this.entries.length - 1, 0));
  }

  /** Applies the marks, scrolling to the selection when it changed or the panel was shown again. Does nothing while hidden. */
  private render(): void {
    if (this.root.hidden) {
      return;
    }
    const currentIndex = this.current ?? -1;
    if (currentIndex !== this.markedCurrent) {
      this.entry(this.markedCurrent)?.classList.remove('current');
      this.entry(currentIndex)?.classList.add('current');
      this.markedCurrent = currentIndex;
    }
    if (this.selected !== this.markedSelected || this.revealPending) {
      this.entry(this.markedSelected)?.classList.remove('selected');
      const chosen = this.entry(this.selected);
      chosen?.classList.add('selected');
      chosen?.scrollIntoView({ block: 'nearest' });
      this.markedSelected = this.selected;
      this.revealPending = false;
    }
  }

  private entry(index: number): HTMLElement | undefined {
    return index >= 0 ? this.entries[index] : undefined;
  }
}
