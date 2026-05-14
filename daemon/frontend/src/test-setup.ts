import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

// jsdom does not implement matchMedia or ResizeObserver; stub them so xterm can initialize in tests.
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

// === Canvas2D stub (jsdom does not implement getContext('2d')) ===
class FakeCanvasRenderingContext2D {
  canvas: HTMLCanvasElement;
  fillStyle: string = '#000';
  strokeStyle: string = '#000';
  font: string = '';
  textBaseline: CanvasTextBaseline = 'alphabetic';
  globalCompositeOperation: GlobalCompositeOperation = 'source-over';
  globalAlpha: number = 1;
  fillText = vi.fn();
  fillRect = vi.fn();
  clearRect = vi.fn();
  strokeRect = vi.fn();
  beginPath = vi.fn();
  moveTo = vi.fn();
  lineTo = vi.fn();
  stroke = vi.fn();
  scale = vi.fn();
  setTransform = vi.fn();
  getTransform = vi.fn(() => ({ a: 1, b: 0, c: 0, d: 1, e: 0, f: 0 }));
  drawImage = vi.fn();
  measureText = vi.fn((s: string) => ({
    width: s.length * 8,
    actualBoundingBoxAscent: 12,
    actualBoundingBoxDescent: 3,
  }));
  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
  }
}

// NOTE: caches one FakeCanvasRenderingContext2D per canvas element to match browser semantics —
// repeated getContext('2d') calls on the same canvas return the same object.
const canvasCtxCache = new WeakMap<HTMLCanvasElement, FakeCanvasRenderingContext2D>();
HTMLCanvasElement.prototype.getContext = vi.fn(function (this: HTMLCanvasElement, ctxId: string) {
  if (ctxId !== '2d') return null;
  let ctx = canvasCtxCache.get(this);
  if (!ctx) {
    ctx = new FakeCanvasRenderingContext2D(this);
    canvasCtxCache.set(this, ctx);
  }
  return ctx as unknown as CanvasRenderingContext2D;
}) as typeof HTMLCanvasElement.prototype.getContext;

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

// === ImageBitmap stub (glyph atlas uses createImageBitmap in browsers) ===
if (typeof (globalThis as { createImageBitmap?: unknown }).createImageBitmap !== 'function') {
  (globalThis as { createImageBitmap: unknown }).createImageBitmap = vi.fn(
    async () => ({ close: vi.fn() }) as unknown as ImageBitmap,
  );
}
