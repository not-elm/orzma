import { afterEach, describe, expect, it } from 'vitest';
import { AddressBar, type AddressHost, type Preview } from './address';
import type { Chrome } from './chrome';

const OMNIBOX =
  '<div class="omnibox"><input class="address-input" /><span class="preview" hidden></span></div>';

function chrome(overrides: Partial<Chrome> = {}): Chrome {
  return {
    mode: 'address',
    url: 'https://example.com/',
    pendingKey: null,
    seed: 'https://example.com/',
    addressEpoch: 1,
    ...overrides,
  };
}

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

const liveBars: AddressBar[] = [];

afterEach(() => {
  for (const bar of liveBars.splice(0)) {
    bar.apply(chrome({ mode: 'normal' }));
  }
});

function setup(host: Partial<AddressHost> = {}) {
  document.body.innerHTML = OMNIBOX;
  const omnibox = document.querySelector('.omnibox') as HTMLElement;
  const calls = { preview: [] as string[], submit: [] as string[], cancel: 0, openAddress: 0 };
  const bar = new AddressBar(omnibox, {
    preview:
      host.preview ??
      ((text) => {
        calls.preview.push(text);
        return Promise.resolve({ kind: 'search', label: 'Search DuckDuckGo' });
      }),
    submit:
      host.submit ??
      ((text) => {
        calls.submit.push(text);
        return Promise.resolve({ kind: 'open', label: 'Open example.com' });
      }),
    cancel: () => {
      calls.cancel += 1;
    },
    openAddress: () => {
      calls.openAddress += 1;
    },
  });
  liveBars.push(bar);
  const input = omnibox.querySelector('input') as HTMLInputElement;
  const preview = omnibox.querySelector('.preview') as HTMLElement;
  return { bar, omnibox, input, preview, calls };
}

function keydown(target: HTMLElement, key: string, isComposing = false, keyCode?: number) {
  const event = new KeyboardEvent('keydown', { key, isComposing, bubbles: true, cancelable: true });
  if (keyCode !== undefined) {
    Object.defineProperty(event, 'keyCode', { value: keyCode });
  }
  target.dispatchEvent(event);
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('AddressBar', () => {
  it('fills, focuses and selects the seed when the address bar opens', async () => {
    const { bar, input, preview, calls } = setup();
    bar.apply(chrome());
    expect(input.value).toBe('https://example.com/');
    expect(document.activeElement).toBe(input);
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, input.value.length]);
    await flush();
    expect(calls.preview).toEqual(['https://example.com/']);
    expect(preview.hidden).toBe(false);
    expect(preview.textContent).toBe('↵ Search DuckDuckGo');
  });

  it('keeps the typed text when the same epoch arrives again', () => {
    const { bar, input } = setup();
    bar.apply(chrome());
    input.value = 'rust async';
    bar.apply(chrome({ url: 'https://example.com/next', seed: 'https://example.com/next' }));
    expect(input.value).toBe('rust async');
  });

  it('refills the input for a new epoch, even without a closed state in between', () => {
    const { bar, input } = setup();
    bar.apply(chrome());
    input.value = 'rust async';
    bar.apply(chrome({ addressEpoch: 2, seed: 'https://docs.rs/' }));
    expect(input.value).toBe('https://docs.rs/');
  });

  it('blurs the input when the address bar closes', () => {
    const { bar, input } = setup();
    bar.apply(chrome());
    bar.apply(chrome({ mode: 'normal' }));
    expect(document.activeElement).not.toBe(input);
  });

  it('submits on Enter and cancels on Esc', () => {
    const { bar, input, calls } = setup();
    bar.apply(chrome());
    input.value = 'rust async';
    keydown(input, 'Enter');
    keydown(input, 'Escape');
    expect(calls.submit).toEqual(['rust async']);
    expect(calls.cancel).toBe(1);
  });

  it('still submits and cancels after focus leaves the input', () => {
    const { bar, input, calls } = setup();
    bar.apply(chrome());
    input.value = 'rust async';
    input.blur();
    keydown(document.body, 'Enter');
    keydown(document.body, 'Escape');
    expect(calls.submit).toEqual(['rust async']);
    expect(calls.cancel).toBe(1);
  });

  it('ignores Enter and Esc outside address mode', () => {
    const { bar, calls } = setup();
    bar.apply(chrome({ mode: 'normal' }));
    keydown(document.body, 'Enter');
    keydown(document.body, 'Escape');
    expect(calls.submit).toEqual([]);
    expect(calls.cancel).toBe(0);
  });

  it('ignores Enter and Esc while an IME composes', () => {
    const { bar, input, calls } = setup();
    bar.apply(chrome());
    keydown(input, 'Enter', true);
    keydown(input, 'Escape', true);
    keydown(input, 'Enter', false, 229);
    expect(calls.submit).toEqual([]);
    expect(calls.cancel).toBe(0);
  });

  it('drops a preview reply that a newer one superseded', async () => {
    const replies = [deferred<Preview>(), deferred<Preview>()];
    let next = 0;
    const { bar, input, preview } = setup({
      preview: () => {
        const reply = replies[next];
        next += 1;
        return reply.promise;
      },
    });
    bar.apply(chrome({ seed: 'rust' }));
    input.value = 'github.com';
    input.dispatchEvent(new InputEvent('input', { isComposing: false }));
    replies[1].resolve({ kind: 'open', label: 'Open github.com' });
    await flush();
    replies[0].resolve({ kind: 'search', label: 'Search DuckDuckGo' });
    await flush();
    expect(preview.textContent).toBe('↵ Open github.com');
  });

  it('marks an invalid submit and keeps the text', async () => {
    const { bar, input, preview } = setup({
      submit: () => Promise.resolve({ kind: 'invalid', label: 'Unsupported scheme: file' }),
    });
    bar.apply(chrome());
    input.value = 'file:///etc';
    keydown(input, 'Enter');
    await flush();
    expect(input.value).toBe('file:///etc');
    expect(preview.textContent).toBe('Unsupported scheme: file');
    expect(preview.classList.contains('rejected')).toBe(true);
  });

  it('keeps the input focused when the rest of the chrome is pressed while editing', () => {
    const { bar, omnibox, input } = setup();
    const press = (target: HTMLElement) => {
      const event = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
      target.dispatchEvent(event);
      return event.defaultPrevented;
    };
    bar.apply(chrome());
    expect(press(omnibox)).toBe(true);
    expect(press(input)).toBe(false);
    bar.apply(chrome({ mode: 'normal' }));
    expect(press(omnibox)).toBe(false);
  });

  it('opens the address bar on an omnibox click only outside address mode', () => {
    const { bar, omnibox, calls } = setup();
    bar.apply(chrome({ mode: 'normal' }));
    omnibox.click();
    expect(calls.openAddress).toBe(1);
    bar.apply(chrome());
    omnibox.click();
    expect(calls.openAddress).toBe(1);
  });
});
