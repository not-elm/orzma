import { type HeadingInfo, headingLabel } from './chrome';

const INDENT_PX = 12;

/** The outline sidebar: one entry per heading, the selection, and the current section. */
export class OutlinePanel {
  private readonly root: HTMLElement;
  private readonly list: HTMLElement;
  private entries: HTMLElement[] = [];
  private selected = -1;
  private current = -1;
  private revealPending = false;

  constructor(root: HTMLElement, onJump: (index: number) => void) {
    this.root = root;
    this.list = root.querySelector('.outline-list') as HTMLElement;
    this.list.addEventListener('click', (e) => {
      const target = e.target instanceof Element ? e.target : null;
      const item = target?.closest<HTMLElement>('[data-index]');
      if (item?.dataset.index !== undefined) {
        onJump(Number(item.dataset.index));
      }
    });
  }

  /** Shows or hides the panel. Once shown again, the next `mark` scrolls to the selection. */
  setOpen(open: boolean): void {
    if (open && this.root.hidden) {
      this.revealPending = true;
    }
    this.root.hidden = !open;
  }

  /** Replaces the entries with `items`, indented relative to the shallowest heading. */
  setItems(items: readonly HeadingInfo[]): void {
    this.selected = -1;
    this.current = -1;
    if (items.length === 0) {
      const empty = document.createElement('li');
      empty.className = 'outline-empty';
      empty.textContent = 'No headings';
      this.entries = [];
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
  }

  /**
   * Marks entry `selected` as the keyboard selection and entry `current` as the section in view,
   * scrolling to the selection when it changed or the panel was shown again. Does nothing while
   * the panel is hidden.
   */
  mark(selected: number, current: number | null): void {
    if (this.root.hidden) {
      return;
    }
    const currentIndex = current ?? -1;
    if (currentIndex !== this.current) {
      this.entry(this.current)?.classList.remove('current');
      this.entry(currentIndex)?.classList.add('current');
      this.current = currentIndex;
    }
    if (selected !== this.selected || this.revealPending) {
      this.entry(this.selected)?.classList.remove('selected');
      const chosen = this.entry(selected);
      chosen?.classList.add('selected');
      chosen?.scrollIntoView({ block: 'nearest' });
      this.selected = selected;
      this.revealPending = false;
    }
  }

  private entry(index: number): HTMLElement | undefined {
    return index >= 0 ? this.entries[index] : undefined;
  }
}
