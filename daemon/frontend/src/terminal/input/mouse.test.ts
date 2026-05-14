import { describe, expect, it } from 'vitest';
import type { FontMetrics } from '../renderer/font';
import { encodeMouseEvent, pointToCell } from './mouse';

const dec = new TextDecoder();

function fakeMetrics(): FontMetrics {
  return { cellW: 8, cellH: 16, baseline: 12, fontCss: '14px monospace', dpr: 1 };
}

describe('encodeMouseEvent', () => {
  it('returns null when no mouse mode is active', () => {
    expect(
      encodeMouseEvent(
        {
          kind: 'down',
          button: 'left',
          col: 0,
          row: 0,
          shift: false,
          alt: false,
          ctrl: false,
          buttonHeld: false,
        },
        new Set(),
      ),
    ).toBeNull();
  });

  it('SGR encodes left-press at (0,0) as \\e[<0;1;1M', () => {
    const bytes = encodeMouseEvent(
      {
        kind: 'down',
        button: 'left',
        col: 0,
        row: 0,
        shift: false,
        alt: false,
        ctrl: false,
        buttonHeld: false,
      },
      new Set(['mouse-sgr-1006', 'mouse-vt200']),
    );
    expect(bytes).not.toBeNull();
    expect(dec.decode(bytes as Uint8Array)).toBe('\x1b[<0;1;1M');
  });

  it('SGR release uses lowercase m and keeps the press button code', () => {
    const bytes = encodeMouseEvent(
      {
        kind: 'up',
        button: 'left',
        col: 4,
        row: 2,
        shift: false,
        alt: false,
        ctrl: false,
        buttonHeld: false,
      },
      new Set(['mouse-sgr-1006', 'mouse-vt200']),
    );
    expect(dec.decode(bytes as Uint8Array)).toBe('\x1b[<0;5;3m');
  });

  it('DEFAULT encoding for left-press at (0,0)', () => {
    const bytes = encodeMouseEvent(
      {
        kind: 'down',
        button: 'left',
        col: 0,
        row: 0,
        shift: false,
        alt: false,
        ctrl: false,
        buttonHeld: false,
      },
      new Set(['mouse-vt200']),
    );
    expect(bytes).not.toBeNull();
    expect(Array.from(bytes as Uint8Array)).toEqual([0x1b, 0x5b, 0x4d, 32, 33, 33]);
  });

  it('DEFAULT encoding suppresses on coord overflow (>223)', () => {
    const bytes = encodeMouseEvent(
      {
        kind: 'down',
        button: 'left',
        col: 224,
        row: 0,
        shift: false,
        alt: false,
        ctrl: false,
        buttonHeld: false,
      },
      new Set(['mouse-vt200']),
    );
    expect(bytes).toBeNull();
  });

  it('modifier bits accumulate (Shift=4, Alt=8, Ctrl=16)', () => {
    const bytes = encodeMouseEvent(
      {
        kind: 'down',
        button: 'left',
        col: 0,
        row: 0,
        shift: true,
        alt: true,
        ctrl: true,
        buttonHeld: false,
      },
      new Set(['mouse-sgr-1006', 'mouse-vt200']),
    );
    expect(dec.decode(bytes as Uint8Array)).toBe('\x1b[<28;1;1M');
  });

  it('motion with buttonHeld=false is dropped under mouse-btn-event', () => {
    const bytes = encodeMouseEvent(
      {
        kind: 'move',
        button: 'none',
        col: 1,
        row: 1,
        shift: false,
        alt: false,
        ctrl: false,
        buttonHeld: false,
      },
      new Set(['mouse-btn-event']),
    );
    expect(bytes).toBeNull();
  });

  it('motion with buttonHeld=true is emitted under mouse-btn-event', () => {
    const bytes = encodeMouseEvent(
      {
        kind: 'move',
        button: 'left',
        col: 1,
        row: 1,
        shift: false,
        alt: false,
        ctrl: false,
        buttonHeld: true,
      },
      new Set(['mouse-sgr-1006', 'mouse-btn-event']),
    );
    expect(dec.decode(bytes as Uint8Array)).toBe('\x1b[<32;2;2M');
  });

  it('motion with buttonHeld=false is emitted under mouse-any-event', () => {
    const bytes = encodeMouseEvent(
      {
        kind: 'move',
        button: 'none',
        col: 1,
        row: 1,
        shift: false,
        alt: false,
        ctrl: false,
        buttonHeld: false,
      },
      new Set(['mouse-sgr-1006', 'mouse-any-event']),
    );
    expect(dec.decode(bytes as Uint8Array)).toBe('\x1b[<35;2;2M');
  });

  it('wheel up encodes Cb=64', () => {
    const bytes = encodeMouseEvent(
      {
        kind: 'wheel',
        button: 'wheelUp',
        col: 0,
        row: 0,
        shift: false,
        alt: false,
        ctrl: false,
        buttonHeld: false,
      },
      new Set(['mouse-sgr-1006', 'mouse-vt200']),
    );
    expect(dec.decode(bytes as Uint8Array)).toBe('\x1b[<64;1;1M');
  });

  it('wheel down encodes Cb=65', () => {
    const bytes = encodeMouseEvent(
      {
        kind: 'wheel',
        button: 'wheelDown',
        col: 0,
        row: 0,
        shift: false,
        alt: false,
        ctrl: false,
        buttonHeld: false,
      },
      new Set(['mouse-sgr-1006', 'mouse-vt200']),
    );
    expect(dec.decode(bytes as Uint8Array)).toBe('\x1b[<65;1;1M');
  });
});

describe('pointToCell', () => {
  it('translates clientX/Y via getBoundingClientRect', () => {
    const canvas = document.createElement('canvas');
    canvas.getBoundingClientRect = () =>
      ({
        left: 100,
        top: 50,
        right: 900,
        bottom: 450,
        width: 800,
        height: 400,
        x: 100,
        y: 50,
        toJSON: () => '',
      }) as DOMRect;

    const result = pointToCell(canvas, { clientX: 124, clientY: 82 }, fakeMetrics());
    expect(result).toEqual({ col: 3, row: 2 });
  });

  it('is unaffected by devicePixelRatio (CSS-pixel math)', () => {
    const canvas = document.createElement('canvas');
    canvas.getBoundingClientRect = () =>
      ({
        left: 0,
        top: 0,
        right: 800,
        bottom: 400,
        width: 800,
        height: 400,
        x: 0,
        y: 0,
        toJSON: () => '',
      }) as DOMRect;
    const original = window.devicePixelRatio;
    Object.defineProperty(window, 'devicePixelRatio', { value: 2, configurable: true });

    try {
      const result = pointToCell(canvas, { clientX: 16, clientY: 32 }, fakeMetrics());
      expect(result).toEqual({ col: 2, row: 2 });
    } finally {
      Object.defineProperty(window, 'devicePixelRatio', { value: original, configurable: true });
    }
  });
});
