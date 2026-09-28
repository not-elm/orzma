import { afterEach, describe, expect, it, vi } from 'vitest';
import { FindBox } from './find';
import { Search } from './search';

const BOX =
  '<div id="find" hidden><input class="find-input" />' +
  '<span class="find-case">Aa</span>' +
  '<span class="find-count"><span class="find-wrap" hidden>↻</span><span class="find-num"></span></span>' +
  '<button data-act="prev"></button><button data-act="next"></button><button data-act="close"></button></div>';

function setup(html = '<p data-y="100">foo</p><p data-y="200">foo</p>') {
  document.body.innerHTML = `${BOX}<div id="content">${html}</div>`;
  const root = document.getElementById('find') as HTMLElement;
  const content = document.getElementById('content') as HTMLElement;
  const search = new Search({ paint: () => {}, clear: () => {} }, (range) =>
    Number((range.startContainer.parentElement as HTMLElement).dataset.y ?? 0),
  );
  const sent: string[] = [];
  const queue: (() => void)[] = [];
  const box = new FindBox(
    root,
    search,
    {
      submit: (cause) => sent.push(`submit:${cause}`),
      escape: (cause) => sent.push(`escape:${cause}`),
      close: () => sent.push('close'),
    },
    {
      content,
      schedule: (task) => queue.push(task),
      reveal: () => {},
      scrollY: () => 0,
      scrollTo: () => {},
    },
  );
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
  return { box, root, content, input, sent, queue, flush, type, key, count };
}

afterEach(() => {
  vi.useRealTimers();
});

describe('FindBox', () => {
  it('ignores Enter and Escape while the IME is composing', () => {
    const t = setup();
    t.box.setStage('typing');
    t.type('foo');
    t.flush();
    t.key('Enter', true);
    t.key('Escape', true);
    expect(t.sent).toEqual([]);
  });

  it('submits on Enter only when there is a match', () => {
    const t = setup();
    t.box.setStage('typing');
    t.type('zzz');
    t.flush();
    t.key('Enter');
    expect(t.sent).toEqual([]);
    t.type('foo');
    t.flush();
    t.key('Enter');
    expect(t.sent).toEqual(['submit:key']);
  });

  it('runs a pending search before deciding on Enter', () => {
    const t = setup();
    t.box.setStage('typing');
    t.type('foo');
    t.key('Enter');
    expect(t.sent).toEqual(['submit:key']);
  });

  it('asks to cancel on Escape', () => {
    const t = setup();
    t.box.setStage('typing');
    t.key('Escape');
    expect(t.sent).toEqual(['escape:key']);
  });

  it('replaces the selection when the TUI relays a typed character', () => {
    const t = setup();
    t.input.value = 'old';
    t.box.setStage('typing');
    t.box.typeText('n');
    expect(t.input.value).toBe('n');
    t.box.typeText('o');
    expect(t.input.value).toBe('no');
    t.box.backspace();
    expect(t.input.value).toBe('n');
  });

  it('resolves a blur while typing by the match count', () => {
    const hit = setup();
    hit.box.setStage('typing');
    hit.type('foo');
    hit.flush();
    hit.input.dispatchEvent(new FocusEvent('blur'));
    expect(hit.sent).toEqual(['submit:blur']);

    const miss = setup();
    miss.box.setStage('typing');
    miss.type('zzz');
    miss.flush();
    miss.box.resolve();
    expect(miss.sent).toEqual(['escape:blur']);
  });

  it('sends nothing when the input blurs after the search went active', () => {
    const t = setup();
    t.box.setStage('typing');
    t.type('foo');
    t.flush();
    t.box.setStage('active');
    t.input.dispatchEvent(new FocusEvent('blur'));
    expect(t.sent).toEqual([]);
  });

  it('coalesces a burst of input into one search', () => {
    const t = setup();
    t.box.setStage('typing');
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
    t.box.setStage('typing');
    t.type('zzz');
    t.flush();
    expect(t.count()).toBe('No results');
    expect(t.root.classList.contains('no-results')).toBe(true);
  });

  it('shows the wrap mark for a moment after wrapping', () => {
    vi.useFakeTimers();
    const t = setup();
    t.box.setStage('typing');
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
    t.box.setStage('typing');
    t.type('foo');
    t.flush();
    t.box.nav('next');
    t.content.innerHTML = '<p data-y="0">foo</p>';
    t.box.rerun();
    expect(t.count()).toBe('1 / 1');
  });

  it('searches the previous query again when reopened', () => {
    const t = setup();
    t.box.setStage('typing');
    t.type('foo');
    t.flush();
    t.box.setStage('active');
    t.box.setStage('closed');
    t.box.setStage('typing');
    expect(t.queue).toHaveLength(1);
    t.flush();
    expect(t.count()).toBe('1 / 2');
  });

  it('closes a confirmed search with the close button', () => {
    const t = setup();
    t.box.setStage('typing');
    t.type('foo');
    t.flush();
    t.box.setStage('active');
    (t.root.querySelector('[data-act="close"]') as HTMLElement).click();
    expect(t.sent).toEqual(['close']);
  });

  it('keeps the input read-only outside typing', () => {
    const t = setup();
    expect(t.input.readOnly).toBe(true);
    t.box.setStage('typing');
    expect(t.input.readOnly).toBe(false);
    t.box.setStage('active');
    expect(t.input.readOnly).toBe(true);
  });
});
