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
  fillStyle: string = '#000';
  strokeStyle: string = '#000';
  font: string = '';
  textBaseline: CanvasTextBaseline = 'alphabetic';
  globalCompositeOperation: GlobalCompositeOperation = 'source-over';
  fillText = vi.fn();
  fillRect = vi.fn();
  clearRect = vi.fn();
  strokeRect = vi.fn();
  beginPath = vi.fn();
  moveTo = vi.fn();
  lineTo = vi.fn();
  stroke = vi.fn();
  scale = vi.fn();
  drawImage = vi.fn();
  measureText = vi.fn((s: string) => ({
    width: s.length * 8,
    actualBoundingBoxAscent: 12,
    actualBoundingBoxDescent: 3,
  }));
}

HTMLCanvasElement.prototype.getContext = vi.fn(function (this: HTMLCanvasElement, ctxId: string) {
  return ctxId === '2d'
    ? (new FakeCanvasRenderingContext2D() as unknown as CanvasRenderingContext2D)
    : null;
}) as typeof HTMLCanvasElement.prototype.getContext;

// === ImageBitmap stub (glyph atlas uses createImageBitmap in browsers) ===
if (typeof (globalThis as { createImageBitmap?: unknown }).createImageBitmap !== 'function') {
  (globalThis as { createImageBitmap: unknown }).createImageBitmap = vi.fn(
    async () => ({ close: vi.fn() }) as unknown as ImageBitmap,
  );
}
