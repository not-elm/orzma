import { describe, expect, it, vi } from 'vitest';
import { createGrid } from '../renderer/grid';
import { setupPointerOverlays } from './pointer-overlays';

const fakeFm = { cellW: 8, cellH: 16, baseline: 12, fontCss: '14px monospace', dpr: 1 };

function makeRefs() {
  const target = document.createElement('div');
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
  document.body.appendChild(target);
  return { target, canvas };
}

describe('setupPointerOverlays', () => {
  it('starts a selection on pointerdown when no mouse mode is active', () => {
    const { target, canvas } = makeRefs();
    const grid = createGrid({ cols: 80, rows: 24 });
    const setSelection = vi.fn();
    const setLinkHover = vi.fn();

    const cleanup = setupPointerOverlays(
      target,
      canvas,
      { current: fakeFm },
      { current: new Set<string>() },
      { current: grid },
      { current: new Map<number, string>() },
      setSelection,
      setLinkHover,
    );

    target.dispatchEvent(
      new PointerEvent('pointerdown', {
        button: 0,
        pointerId: 1,
        clientX: 16,
        clientY: 32,
      }),
    );
    expect(setSelection).toHaveBeenCalledWith({
      anchor: { col: 2, row: 2 },
      head: { col: 2, row: 2 },
    });

    cleanup();
    document.body.removeChild(target);
  });

  it('skips selection start when mouse mode is active and shift not held', () => {
    const { target, canvas } = makeRefs();
    const grid = createGrid({ cols: 80, rows: 24 });
    const setSelection = vi.fn();
    const setLinkHover = vi.fn();

    const cleanup = setupPointerOverlays(
      target,
      canvas,
      { current: fakeFm },
      { current: new Set(['mouse-vt200']) },
      { current: grid },
      { current: new Map<number, string>() },
      setSelection,
      setLinkHover,
    );

    target.dispatchEvent(
      new PointerEvent('pointerdown', {
        button: 0,
        pointerId: 1,
        shiftKey: false,
        clientX: 16,
        clientY: 32,
      }),
    );
    expect(setSelection).not.toHaveBeenCalled();

    cleanup();
    document.body.removeChild(target);
  });

  it('sets linkHover when pointer moves over a cell with hyperlinkId', async () => {
    const { target, canvas } = makeRefs();
    const grid = createGrid({ cols: 80, rows: 24 });
    grid.cells[2] = [
      { text: 'a', width: 1, fg: null, bg: null, style: 0 },
      { text: 'b', width: 1, fg: null, bg: null, style: 0, hyperlinkId: 7 },
      { text: 'c', width: 1, fg: null, bg: null, style: 0, hyperlinkId: 7 },
    ];
    grid.rowVersions[2] = 5;

    const setSelection = vi.fn();
    const setLinkHover = vi.fn();

    const cleanup = setupPointerOverlays(
      target,
      canvas,
      { current: fakeFm },
      { current: new Set<string>() },
      { current: grid },
      { current: new Map([[7, 'https://example.com']]) },
      setSelection,
      setLinkHover,
    );

    target.dispatchEvent(
      new PointerEvent('pointermove', {
        pointerId: 1,
        clientX: 1 * 8 + 1,
        clientY: 2 * 16 + 1,
      }),
    );
    await new Promise((r) => requestAnimationFrame(() => r(null)));
    expect(setLinkHover).toHaveBeenCalled();
    const callArg = setLinkHover.mock.calls[0]?.[0];
    expect(callArg).toMatchObject({
      uri: 'https://example.com',
      row: 2,
    });

    cleanup();
    document.body.removeChild(target);
  });

  it('updates selection.head on pointermove during a drag', async () => {
    const { target, canvas } = makeRefs();
    const grid = createGrid({ cols: 80, rows: 24 });
    const setSelection = vi.fn();
    const setLinkHover = vi.fn();

    const cleanup = setupPointerOverlays(
      target,
      canvas,
      { current: fakeFm },
      { current: new Set<string>() },
      { current: grid },
      { current: new Map<number, string>() },
      setSelection,
      setLinkHover,
    );

    target.dispatchEvent(
      new PointerEvent('pointerdown', {
        button: 0,
        pointerId: 1,
        clientX: 8,
        clientY: 16,
      }),
    );
    target.dispatchEvent(
      new PointerEvent('pointermove', {
        pointerId: 1,
        clientX: 32,
        clientY: 16,
      }),
    );
    await new Promise((r) => requestAnimationFrame(() => r(null)));

    const last = setSelection.mock.calls.at(-1)?.[0];
    expect(last).toEqual({
      anchor: { col: 1, row: 1 },
      head: { col: 4, row: 1 },
    });

    cleanup();
    document.body.removeChild(target);
  });
});
