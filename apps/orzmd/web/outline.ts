import { type HeadingInfo, headingLabel } from './chrome';

const INDENT_PX = 12;

/** The outline sidebar: one entry per heading, the selection, and the current section. */
export class OutlinePanel {
  private readonly list: HTMLElement;
  private lastSelected = -1;

  constructor(root: HTMLElement, onJump: (index: number) => void) {
    this.list = root.querySelector('.outline-list') as HTMLElement;
    this.list.addEventListener('click', (e) => {
      const target = e.target instanceof Element ? e.target : null;
      const item = target?.closest<HTMLElement>('[data-index]');
      if (item?.dataset.index !== undefined) {
        onJump(Number(item.dataset.index));
      }
    });
  }

  /** Replaces the entries with `items`, indented relative to the shallowest heading. */
  setItems(items: readonly HeadingInfo[]): void {
    this.lastSelected = -1;
    if (items.length === 0) {
      const empty = document.createElement('li');
      empty.className = 'outline-empty';
      empty.textContent = 'No headings';
      this.list.replaceChildren(empty);
      return;
    }
    const shallowest = Math.min(...items.map((h) => h.level));
    this.list.replaceChildren(
      ...items.map((heading, index) => {
        const li = document.createElement('li');
        li.dataset.index = String(index);
        li.style.paddingLeft = `${INDENT_PX * (1 + heading.level - shallowest)}px`;
        li.textContent = headingLabel(heading.text);
        return li;
      }),
    );
  }

  /**
   * Marks entry `selected` as the keyboard selection and entry `current` as the section in view.
   * The panel scrolls to the selection only when the selection changed.
   */
  mark(selected: number, current: number | null): void {
    let chosen: HTMLElement | null = null;
    for (const li of this.list.querySelectorAll<HTMLElement>('[data-index]')) {
      const index = Number(li.dataset.index);
      li.classList.toggle('selected', index === selected);
      li.classList.toggle('current', index === current);
      if (index === selected) {
        chosen = li;
      }
    }
    if (chosen !== null && selected !== this.lastSelected) {
      chosen.scrollIntoView({ block: 'nearest' });
    }
    this.lastSelected = selected;
  }
}
