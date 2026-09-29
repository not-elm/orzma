import { beforeAll, describe, expect, it } from 'vitest';
import { OutlinePanel } from './outline';

beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
});

const ABC = [
  { level: 1, text: 'A' },
  { level: 1, text: 'B' },
  { level: 1, text: 'C' },
];

function panel() {
  const root = document.createElement('nav');
  root.hidden = true;
  root.innerHTML = '<ol class="outline-list"></ol>';
  const jumps: number[] = [];
  const layouts: boolean[] = [];
  const p = new OutlinePanel(root, {
    jump: (index) => jumps.push(index),
    relayout: (open) => layouts.push(open),
  });
  const marked = (cls: string) =>
    Array.from(root.querySelectorAll(`.${cls}`)).map((li) => li.textContent);
  return { root, panel: p, jumps, layouts, marked };
}

describe('OutlinePanel', () => {
  it('lists headings indented from the shallowest level', () => {
    const { root, panel: p } = panel();
    p.setItems([
      { level: 2, text: 'Settings' },
      { level: 3, text: '[orzma]' },
    ]);
    const items = root.querySelectorAll<HTMLElement>('[data-index]');
    expect(Array.from(items).map((li) => li.textContent)).toEqual(['Settings', '[orzma]']);
    expect(items[0].style.paddingLeft).toBe('12px');
    expect(items[1].style.paddingLeft).toBe('24px');
  });

  it('says so when the document has no headings', () => {
    const { root, panel: p } = panel();
    p.setItems([]);
    expect(root.querySelector('.outline-empty')?.textContent).toBe('No headings');
    expect(root.querySelectorAll('[data-index]')).toHaveLength(0);
  });

  it('labels a heading without text', () => {
    const { root, panel: p } = panel();
    p.setItems([{ level: 1, text: '' }]);
    expect(root.querySelector('[data-index]')?.textContent).toBe('(untitled)');
  });

  it('opens on the heading being read and lays the page out for it', () => {
    const t = panel();
    t.panel.setItems(ABC);
    t.panel.open(1);
    expect(t.panel.isOpen()).toBe(true);
    expect(t.root.hidden).toBe(false);
    expect(t.layouts).toEqual([true]);
    expect(t.marked('selected')).toEqual(['B']);
  });

  it('opens on the first heading above the first one, and on the last past the end', () => {
    const t = panel();
    t.panel.setItems(ABC);
    t.panel.open(null);
    expect(t.marked('selected')).toEqual(['A']);
    t.panel.close();
    t.panel.open(9);
    expect(t.marked('selected')).toEqual(['C']);
  });

  it('does nothing when opened while open or closed while closed', () => {
    const t = panel();
    t.panel.setItems(ABC);
    t.panel.close();
    t.panel.open(0);
    t.panel.open(2);
    expect(t.marked('selected')).toEqual(['A']);
    t.panel.close();
    t.panel.close();
    expect(t.layouts).toEqual([true, false]);
    expect(t.panel.isOpen()).toBe(false);
  });

  it('marks the selection and the current section separately', () => {
    const t = panel();
    t.panel.setItems(ABC);
    t.panel.open(1);
    t.panel.markCurrent(0);
    expect(t.marked('current')).toEqual(['A']);
    expect(t.marked('selected')).toEqual(['B']);
  });

  it('marks nothing while closed', () => {
    const t = panel();
    t.panel.setItems(ABC);
    t.panel.markCurrent(0);
    expect(t.marked('current')).toEqual([]);
    t.panel.open(0);
    expect(t.marked('current')).toEqual(['A']);
  });

  it('moves the selection and stops at both ends', () => {
    const t = panel();
    t.panel.setItems(ABC);
    t.panel.open(0);
    t.panel.move(-1);
    expect(t.marked('selected')).toEqual(['A']);
    t.panel.move(1);
    t.panel.move(1);
    t.panel.move(1);
    expect(t.marked('selected')).toEqual(['C']);
  });

  it('jumps to the selection', () => {
    const t = panel();
    t.panel.setItems(ABC);
    t.panel.open(0);
    t.panel.move(1);
    t.panel.choose();
    expect(t.jumps).toEqual([1]);
  });

  it('does nothing without headings', () => {
    const t = panel();
    t.panel.setItems([]);
    t.panel.open(null);
    t.panel.move(1);
    t.panel.move(-1);
    t.panel.choose();
    expect(t.jumps).toEqual([]);
    expect(t.marked('selected')).toEqual([]);
  });

  it('selects and jumps to a clicked heading', () => {
    const t = panel();
    t.panel.setItems(ABC);
    t.panel.open(0);
    (t.root.querySelectorAll<HTMLElement>('[data-index]')[1] as HTMLElement).click();
    expect(t.jumps).toEqual([1]);
    expect(t.marked('selected')).toEqual(['B']);
  });

  it('keeps the selection within the new headings', () => {
    const t = panel();
    t.panel.setItems(ABC);
    t.panel.open(2);
    t.panel.setItems(ABC.slice(0, 2));
    expect(t.marked('selected')).toEqual(['B']);
  });

  it('scrolls to an unchanged selection once the panel is shown again', () => {
    const t = panel();
    t.panel.setItems(ABC);
    const scrolled: string[] = [];
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this.textContent ?? '');
    };
    t.panel.open(1);
    t.panel.close();
    t.panel.open(1);
    t.panel.markCurrent(null);
    Element.prototype.scrollIntoView = () => {};
    expect(scrolled).toEqual(['B', 'B']);
  });

  it('moves the marks off the entries marked before', () => {
    const t = panel();
    t.panel.setItems(ABC);
    t.panel.open(0);
    t.panel.markCurrent(0);
    t.panel.move(1);
    t.panel.move(1);
    t.panel.markCurrent(1);
    expect(t.marked('selected')).toEqual(['C']);
    expect(t.marked('current')).toEqual(['B']);
    t.panel.markCurrent(null);
    expect(t.marked('current')).toEqual([]);
  });
});
