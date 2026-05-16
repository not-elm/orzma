import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

// jsdom does not implement matchMedia or ResizeObserver; stub them so the
// DOM renderer's media-query / layout-observer paths work in tests.
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  }),
});

if (typeof globalThis.ResizeObserver === 'undefined') {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

// NOTE: jsdom does not implement scrollIntoView; stub it so any component
// that calls element.scrollIntoView() does not throw in tests.
Element.prototype.scrollIntoView = vi.fn();

// === getBoundingClientRect stub (jsdom does not perform layout) ===
// NOTE: DOM probe functions (cellWidthOf, measureGlyph) rely on getBoundingClientRect
// to measure rendered glyph widths. jsdom returns all-zero DOMRects, so we stub it to
// return a fixed non-zero width so tests can assert `> 0`.
HTMLElement.prototype.getBoundingClientRect = vi.fn(function (this: HTMLElement) {
  const textLength = (this.textContent ?? '').length;
  return {
    width: textLength > 0 ? textLength * 8 : 0,
    height: 14,
    top: 0,
    left: 0,
    bottom: 14,
    right: textLength * 8,
    x: 0,
    y: 0,
    toJSON: () => ({}),
  } as DOMRect;
});
