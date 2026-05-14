import { render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { __resetGlyphWidthCacheForTests } from './font';
import type { Cell } from './grid';
import { Row } from './Row';

const fakeFm = { cellW: 8, cellH: 16, baseline: 12, fontCss: '14px monospace', dpr: 1 };
const noHyperlinks = new Map<number, string>();

function makeCell(over: Partial<Cell> = {}): Cell {
  return {
    text: 'a',
    width: 1,
    fg: null,
    bg: null,
    style: 0,
    ...over,
  };
}

let container: HTMLElement;
beforeEach(() => {
  container = document.createElement('div');
  container.className = 'font-mono';
  document.body.appendChild(container);
});
afterEach(() => {
  __resetGlyphWidthCacheForTests();
  document.body.removeChild(container);
});

describe('Row basic structure', () => {
  it('renders one <span> per attribute run, skipping width=0 cells', () => {
    const cells: Cell[] = [
      makeCell({ text: 'a', width: 1 }),
      makeCell({ text: 'b', width: 1 }),
      makeCell({ text: '', width: 0 }), // combining mark in its own cell — skip
      makeCell({ text: 'c', width: 1 }),
    ];
    const { container: out } = render(
      <Row cells={cells} version={1} fm={fakeFm} hyperlinks={noHyperlinks} probeRef={container} />,
    );
    const spans = out.querySelectorAll('span');
    expect(spans.length).toBeGreaterThan(0);
    const joined = Array.from(spans)
      .map((s) => s.textContent)
      .join('');
    expect(joined).toBe('abc');
  });

  it('row container has pointer-events-none + height in px', () => {
    const { container: out } = render(
      <Row
        cells={[makeCell()]}
        version={1}
        fm={fakeFm}
        hyperlinks={noHyperlinks}
        probeRef={container}
      />,
    );
    const row = out.firstElementChild as HTMLElement;
    expect(row.className).toContain('pointer-events-none');
    expect(row.style.height).toBe('16px');
  });

  it('wide char (width=2) goes into its own <span> with letter-spacing applied', () => {
    const cells: Cell[] = [makeCell({ text: '日', width: 2 })];
    const { container: out } = render(
      <Row cells={cells} version={1} fm={fakeFm} hyperlinks={noHyperlinks} probeRef={container} />,
    );
    const spans = out.querySelectorAll('span');
    // 1 grapheme 1 span invariant (M5)
    expect(spans.length).toBe(1);
    const span = spans[0] as HTMLElement;
    expect(span.textContent).toBe('日');
    // letterSpacing is set (value depends on probe measurement, just assert non-empty)
    expect(span.style.letterSpacing).toMatch(/px$/);
  });

  it('coalesces consecutive same-attribute cells into one <span>', () => {
    const cells: Cell[] = [
      makeCell({ text: 'a', style: 0 }),
      makeCell({ text: 'b', style: 0 }),
      makeCell({ text: 'c', style: 0 }),
    ];
    const { container: out } = render(
      <Row cells={cells} version={1} fm={fakeFm} hyperlinks={noHyperlinks} probeRef={container} />,
    );
    const spans = out.querySelectorAll('span');
    expect(spans.length).toBe(1);
    expect(spans[0].textContent).toBe('abc');
  });

  it('splits runs at style change', () => {
    const cells: Cell[] = [
      makeCell({ text: 'a', style: 0 }),
      makeCell({ text: 'b', style: 1 /* BOLD */ }),
    ];
    const { container: out } = render(
      <Row cells={cells} version={1} fm={fakeFm} hyperlinks={noHyperlinks} probeRef={container} />,
    );
    const spans = out.querySelectorAll('span');
    expect(spans.length).toBe(2);
    expect(spans[1].className).toContain('font-bold');
  });

  it('applies italic / underline / DIM via class + inline color', () => {
    // ITALIC | UNDERLINE | DIM (2 | 4 | 32 = 38 = 0b100110)
    const cells: Cell[] = [makeCell({ text: 'x', style: 0b100110, fg: 1 })];
    const { container: out } = render(
      <Row cells={cells} version={1} fm={fakeFm} hyperlinks={noHyperlinks} probeRef={container} />,
    );
    const span = out.querySelector('span') as HTMLElement;
    expect(span.className).toContain('italic');
    expect(span.className).toContain('underline');
    // DIM only dims fg via dimColor — span carries an inline color
    expect(span.style.color).toMatch(/rgba\(.*0\.6\)/);
  });

  it('swaps trailing space → NBSP in underlined runs', () => {
    const cells: Cell[] = [
      makeCell({ text: 'a', style: 0b100 /* UNDERLINE */ }),
      makeCell({ text: ' ', style: 0b100 }),
      makeCell({ text: ' ', style: 0b100 }),
    ];
    const { container: out } = render(
      <Row cells={cells} version={1} fm={fakeFm} hyperlinks={noHyperlinks} probeRef={container} />,
    );
    const span = out.querySelector('span') as HTMLElement;
    // All spaces in the underlined run are NBSP (\xa0)
    expect(span.textContent).toBe('a\xa0\xa0');
  });

  it('REVERSE swaps fg and bg classes', () => {
    const cells: Cell[] = [makeCell({ text: 'x', style: 0b10000 /* REVERSE */, fg: 1, bg: 2 })];
    const { container: out } = render(
      <Row cells={cells} version={1} fm={fakeFm} hyperlinks={noHyperlinks} probeRef={container} />,
    );
    const span = out.querySelector('span') as HTMLElement;
    // After swap: fg should derive from bg=2, bg should derive from fg=1
    // For ANSI indexed inputs we expect class "fg-2" + "bg-1"
    expect(span.className).toContain('fg-2');
    expect(span.className).toContain('bg-1');
  });

  it('default fg / bg yield fg-default / bg-default classes (C6)', () => {
    const cells: Cell[] = [makeCell({ text: 'x', fg: null, bg: null })];
    const { container: out } = render(
      <Row cells={cells} version={1} fm={fakeFm} hyperlinks={noHyperlinks} probeRef={container} />,
    );
    const span = out.querySelector('span') as HTMLElement;
    // null = Default
    expect(span.className).toContain('fg-default');
    expect(span.className).toContain('bg-default');
  });
});
