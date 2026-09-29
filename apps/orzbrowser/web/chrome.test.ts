import { describe, expect, it } from 'vitest';
import { type Chrome, renderChrome, splitUrl } from './chrome';

const ROOT =
  '<header id="chrome"><span class="badge"></span><kbd class="pending" hidden></kbd>' +
  '<div class="omnibox"><div class="url-view"><span class="not-secure" hidden>Not secure</span>' +
  '<span class="host"></span><span class="rest"></span></div></div>' +
  '<span class="hints"></span></header>';

function chrome(overrides: Partial<Chrome> = {}): Chrome {
  return {
    mode: 'normal',
    url: 'https://github.com/not-elm/orzma/pulls',
    pendingKey: null,
    seed: '',
    addressEpoch: 0,
    ...overrides,
  };
}

function root(): HTMLElement {
  document.body.innerHTML = ROOT;
  return document.getElementById('chrome') as HTMLElement;
}

function text(el: HTMLElement, selector: string): string | null | undefined {
  return el.querySelector(selector)?.textContent;
}

describe('splitUrl', () => {
  it('splits the host from the path, query and fragment', () => {
    expect(splitUrl('https://github.com/not-elm/orzma/pulls?q=is%3Aopen#top')).toEqual({
      insecure: false,
      host: 'github.com',
      rest: '/not-elm/orzma/pulls?q=is%3Aopen#top',
    });
  });

  it('drops the bare root path', () => {
    expect(splitUrl('https://duckduckgo.com/').rest).toBe('');
  });

  it('keeps a non-default port on the host', () => {
    expect(splitUrl('http://localhost:3000/app').host).toBe('localhost:3000');
  });

  it('flags plain http as insecure', () => {
    expect(splitUrl('http://example.com/').insecure).toBe(true);
    expect(splitUrl('https://example.com/').insecure).toBe(false);
  });

  it('shows a URL that does not parse whole', () => {
    expect(splitUrl('not a url')).toEqual({ insecure: false, host: '', rest: 'not a url' });
  });
});

describe('renderChrome', () => {
  it('labels the badge and keeps exactly one mode class', () => {
    const el = root();
    renderChrome(el, chrome({ mode: 'address' }));
    expect(text(el, '.badge')).toBe('OPEN');
    expect(el.classList.contains('mode-address')).toBe(true);
    renderChrome(el, chrome({ mode: 'insert' }));
    expect(text(el, '.badge')).toBe('INSERT');
    expect(el.classList.contains('mode-address')).toBe(false);
    expect(el.classList.contains('mode-insert')).toBe(true);
  });

  it('draws the host and the rest of the URL', () => {
    const el = root();
    renderChrome(el, chrome());
    expect(text(el, '.host')).toBe('github.com');
    expect(text(el, '.rest')).toBe('/not-elm/orzma/pulls');
  });

  it('shows Not secure only for plain http', () => {
    const el = root();
    const notSecure = el.querySelector('.not-secure') as HTMLElement;
    renderChrome(el, chrome({ url: 'http://example.com/' }));
    expect(notSecure.hidden).toBe(false);
    renderChrome(el, chrome());
    expect(notSecure.hidden).toBe(true);
  });

  it('shows the pending key only while a chord is pending', () => {
    const el = root();
    const pending = el.querySelector('.pending') as HTMLElement;
    renderChrome(el, chrome({ pendingKey: 'g' }));
    expect(pending.hidden).toBe(false);
    expect(pending.textContent).toBe('g');
    renderChrome(el, chrome());
    expect(pending.hidden).toBe(true);
  });

  it('lists the keys of the current mode', () => {
    const el = root();
    const keys = () => [...el.querySelectorAll('.hints kbd')].map((k) => k.textContent);
    renderChrome(el, chrome());
    expect(keys()).toEqual(['o', 'f', '?']);
    renderChrome(el, chrome({ mode: 'address' }));
    expect(keys()).toEqual(['esc']);
    expect(text(el, '.hints')).toBe('esc cancel');
    renderChrome(el, chrome({ mode: 'help' }));
    expect(keys()).toEqual([]);
  });
});
