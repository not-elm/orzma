import { beforeAll, describe, expect, it } from 'vitest';
import { OutlinePanel } from './outline';

beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
});

function panel(onJump: (index: number) => void = () => {}) {
  const root = document.createElement('nav');
  root.innerHTML = '<ol class="outline-list"></ol>';
  return { root, panel: new OutlinePanel(root, onJump) };
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

  it('marks the selection and the current section separately', () => {
    const { root, panel: p } = panel();
    p.setItems([
      { level: 1, text: 'A' },
      { level: 1, text: 'B' },
    ]);
    p.mark(1, 0);
    const [a, b] = Array.from(root.querySelectorAll<HTMLElement>('[data-index]'));
    expect(a.classList.contains('current')).toBe(true);
    expect(a.classList.contains('selected')).toBe(false);
    expect(b.classList.contains('selected')).toBe(true);
  });

  it('reports the clicked heading', () => {
    const jumps: number[] = [];
    const { root, panel: p } = panel((i) => jumps.push(i));
    p.setItems([
      { level: 1, text: 'A' },
      { level: 1, text: 'B' },
    ]);
    (root.querySelectorAll<HTMLElement>('[data-index]')[1] as HTMLElement).click();
    expect(jumps).toEqual([1]);
  });
});
