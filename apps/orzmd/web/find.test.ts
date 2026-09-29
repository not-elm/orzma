import { afterEach, describe, expect, it, vi } from 'vitest';
import { FindBox } from './find';
import { Search } from './search';

const BOX =
  '<div id="find" hidden><input class="find-input" />' +
  '<span class="find-case">Aa</span>' +
  '<span class="find-count"><span class="find-wrap" hidden>↻</span><span class="find-num"></span></span>' +
  '<button data-act="prev"></button><button data-act="next"></button><button data-act="close"></button></div>';

function setup(html = '<p data-y="100">foo</p><p data-y="200">foo</p>', scrollY = 0) {
  document.body.innerHTML = `${BOX}<div id="content">${html}</div>`;
  const root = document.getElementById('find') as HTMLElement;
  const content = document.getElementById('content') as HTMLElement;
  const search = new Search({ paint: () => {}, paintCurrent: () => {}, clear: () => {} }, (range) =>
    Number((range.startContainer.parentElement as HTMLElement).dataset.y ?? 0),
  );
  const scrolled: number[] = [];
  const queue: (() => void)[] = [];
  const box = new FindBox(root, search, {
    content,
    schedule: (task) => queue.push(task),
    reveal: () => {},
    scrollY: () => scrollY,
    scrollTo: (y) => scrolled.push(y),
  });
  const input = root.querySelector('input') as HTMLInputElement;
  const flush = () => {
    while (queue.length > 0) queue.shift()?.();
  };
  const type = (value: string) => {
    input.value = value;
    input.dispatchEvent(new InputEvent('input', { isComposing: false }));
  };
  const key = (k: string, isComposing = false) =>
    input.dispatchEvent(new KeyboardEvent('keydown', { key: k, isComposing }));
  const count = () => root.querySelector('.find-num')?.textContent;
  const button = (act: string) =>
    (root.querySelector(`[data-act="${act}"]`) as HTMLElement).click();
  return { box, root, content, input, scrolled, queue, flush, type, key, count, button };
}

type Setup = ReturnType<typeof setup>;

afterEach(() => {
  vi.useRealTimers();
});

describe('FindBox', () => {
  it('starts closed with a read-only input', () => {
    const t = setup();
    expect(t.box.stage).toBe('closed');
    expect(t.input.readOnly).toBe(true);
  });

  it('opens for typing with the input focused', () => {
    const t = setup();
    t.box.open();
    expect(t.box.stage).toBe('typing');
    expect(t.root.hidden).toBe(false);
    expect(t.input.readOnly).toBe(false);
    expect(document.activeElement).toBe(t.input);
  });

  it('ignores Enter and Escape while the IME is composing', () => {
    const t = setup();
    t.box.open();
    t.type('foo');
    t.flush();
    t.key('Enter', true);
    t.key('Escape', true);
    expect(t.box.stage).toBe('typing');
  });

  it('confirms on Enter only when there is a match', () => {
    const t = setup();
    t.box.open();
    t.type('zzz');
    t.flush();
    t.key('Enter');
    expect(t.box.stage).toBe('typing');
    t.type('foo');
    t.flush();
    t.key('Enter');
    expect(t.box.stage).toBe('active');
    expect(t.root.hidden).toBe(false);
  });

  it('runs a pending search before deciding on Enter', () => {
    const t = setup();
    t.box.open();
    t.type('foo');
    t.key('Enter');
    expect(t.box.stage).toBe('active');
  });

  it('closes on Escape and returns to where the search started', () => {
    const t = setup(undefined, 300);
    t.box.open();
    t.type('foo');
    t.flush();
    t.key('Escape');
    expect(t.box.stage).toBe('closed');
    expect(t.root.hidden).toBe(true);
    expect(t.scrolled).toEqual([300]);
    expect(t.count()).toBe('');
  });

  it('replaces the selection when the TUI relays a typed character', () => {
    const t = setup();
    t.input.value = 'old';
    t.box.open();
    t.box.typeText('n');
    expect(t.input.value).toBe('n');
    t.box.typeText('o');
    expect(t.input.value).toBe('no');
    t.box.backspace();
    expect(t.input.value).toBe('n');
  });

  it('ends a typed search on blur by its match count and leaves the page where it is', () => {
    const hit = setup(undefined, 300);
    hit.box.open();
    hit.type('foo');
    hit.flush();
    hit.input.dispatchEvent(new FocusEvent('blur'));
    expect(hit.box.stage).toBe('active');

    const miss = setup(undefined, 300);
    miss.box.open();
    miss.type('zzz');
    miss.flush();
    miss.input.dispatchEvent(new FocusEvent('blur'));
    expect(miss.box.stage).toBe('closed');
    expect(miss.scrolled).toEqual([]);
  });

  it('takes the focus off the input on every way out of typing', () => {
    const exits: [string, (t: Setup) => void][] = [
      ['Enter', (t) => t.key('Enter')],
      ['Escape', (t) => t.key('Escape')],
      ['blur', (t) => t.input.dispatchEvent(new FocusEvent('blur'))],
      ['close button', (t) => t.button('close')],
      ['reset', (t) => t.box.reset()],
    ];
    for (const [name, exit] of exits) {
      const t = setup();
      t.box.open();
      t.type('foo');
      t.flush();
      expect(document.activeElement, name).toBe(t.input);
      exit(t);
      expect(document.activeElement, name).not.toBe(t.input);
    }
  });

  it('coalesces a burst of input into one search', () => {
    const t = setup();
    t.box.open();
    t.flush();
    t.type('f');
    t.type('fo');
    t.type('foo');
    expect(t.queue).toHaveLength(1);
    t.flush();
    expect(t.count()).toBe('1 / 2');
  });

  it('shows No results for an unmatched query', () => {
    const t = setup();
    t.box.open();
    t.type('zzz');
    t.flush();
    expect(t.count()).toBe('No results');
    expect(t.root.classList.contains('no-results')).toBe(true);
  });

  it('ignores next and prev while closed', () => {
    const t = setup();
    t.box.open();
    t.type('foo');
    t.flush();
    t.key('Escape');
    t.box.nav('next');
    t.box.nav('prev');
    expect(t.count()).toBe('');
    expect(t.root.classList.contains('no-results')).toBe(false);
  });

  it('shows the wrap mark for a moment after wrapping', () => {
    vi.useFakeTimers();
    const t = setup();
    t.box.open();
    t.type('foo');
    t.flush();
    t.box.nav('next');
    t.box.nav('next');
    const wrap = t.root.querySelector('.find-wrap') as HTMLElement;
    expect(wrap.hidden).toBe(false);
    vi.advanceTimersByTime(1500);
    expect(wrap.hidden).toBe(true);
  });

  it('keeps the count after the document re-renders with fewer matches', () => {
    const t = setup();
    t.box.open();
    t.type('foo');
    t.flush();
    t.box.nav('next');
    t.content.innerHTML = '<p data-y="0">foo</p>';
    t.box.rerun();
    expect(t.count()).toBe('1 / 1');
  });

  it('searches the previous query again when reopened', () => {
    const t = setup();
    t.box.open();
    t.type('foo');
    t.flush();
    t.key('Enter');
    t.box.clearHighlights();
    expect(t.box.stage).toBe('closed');
    t.box.open();
    expect(t.queue).toHaveLength(1);
    t.flush();
    expect(t.count()).toBe('1 / 2');
  });

  it('closes a typed search with the close button as Escape does', () => {
    const t = setup(undefined, 300);
    t.box.open();
    t.button('close');
    expect(t.box.stage).toBe('closed');
    expect(t.scrolled).toEqual([300]);
  });

  it('closes a confirmed search with the close button without moving the page', () => {
    const t = setup(undefined, 300);
    t.box.open();
    t.type('foo');
    t.key('Enter');
    t.button('close');
    expect(t.box.stage).toBe('closed');
    expect(t.root.hidden).toBe(true);
    expect(t.scrolled).toEqual([]);
  });

  it('takes no input once the typed search ended', () => {
    const t = setup();
    t.box.open();
    t.type('foo');
    t.key('Enter');
    t.type('foon');
    t.box.typeText('x');
    t.box.backspace();
    t.key('Escape');
    t.flush();
    expect(t.box.stage).toBe('active');
    expect(t.count()).toBe('1 / 2');
    expect(t.input.readOnly).toBe(true);
    t.box.open();
    expect(t.input.readOnly).toBe(false);
  });

  it('clears a confirmed search but leaves a typed one alone', () => {
    const t = setup();
    t.box.open();
    t.box.clearHighlights();
    expect(t.box.stage).toBe('typing');
    t.type('foo');
    t.key('Enter');
    t.box.clearHighlights();
    expect(t.box.stage).toBe('closed');
    expect(t.root.hidden).toBe(true);
  });

  it('closes from any stage on reset without moving the page', () => {
    const typing = setup(undefined, 300);
    typing.box.open();
    typing.box.reset();
    expect(typing.box.stage).toBe('closed');
    expect(typing.scrolled).toEqual([]);

    const active = setup(undefined, 300);
    active.box.open();
    active.type('foo');
    active.key('Enter');
    active.box.reset();
    expect(active.box.stage).toBe('closed');
    expect(active.scrolled).toEqual([]);
    expect(active.count()).toBe('');
  });
});
