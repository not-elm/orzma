import { beforeEach, describe, expect, it } from 'vitest';
import { Search } from './search';

function container(html: string): HTMLElement {
  const el = document.createElement('div');
  el.innerHTML = html;
  document.body.replaceChildren(el);
  return el;
}

describe('Search', () => {
  let search: Search;
  beforeEach(() => {
    search = new Search();
  });

  it('counts case-insensitive matches and wraps them in marks', () => {
    const el = container('<p>foo Foo FOO bar</p>');
    const { total, current } = search.run(el, 'foo');
    expect(total).toBe(3);
    expect(current).toBe(1);
    expect(el.querySelectorAll('mark.orzmd-match').length).toBe(3);
  });

  it('navigates next and wraps around', () => {
    const el = container('<p>x x x</p>');
    search.run(el, 'x');
    expect(search.navigate('next').current).toBe(2);
    expect(search.navigate('next').current).toBe(3);
    expect(search.navigate('next').current).toBe(1);
  });

  it('navigates prev and wraps around', () => {
    const el = container('<p>x x x</p>');
    search.run(el, 'x');
    expect(search.navigate('prev').current).toBe(3);
  });

  it('clear removes all marks', () => {
    const el = container('<p>aaa</p>');
    search.run(el, 'a');
    search.clear(el);
    expect(el.querySelectorAll('mark.orzmd-match').length).toBe(0);
  });

  it('empty query yields zero matches', () => {
    const el = container('<p>anything</p>');
    expect(search.run(el, '').total).toBe(0);
  });

  it('matches across multiple elements', () => {
    const el = container('<p>cat</p><div><span>cat</span></div>');
    expect(search.run(el, 'cat').total).toBe(2);
  });

  it('re-running clears the previous query before highlighting the new one', () => {
    const el = container('<p>foo bar</p>');
    search.run(el, 'foo');
    const result = search.run(el, 'bar');
    expect(result.total).toBe(1);
    const marks = el.querySelectorAll('mark.orzmd-match');
    expect(marks.length).toBe(1);
    expect(marks[0].textContent).toBe('bar');
  });

  it('clear restores the original text', () => {
    const el = container('<p>aaa</p>');
    search.run(el, 'a');
    search.clear(el);
    expect(el.textContent).toBe('aaa');
  });
});
