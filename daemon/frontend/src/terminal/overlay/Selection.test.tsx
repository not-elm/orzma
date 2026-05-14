import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Selection } from './Selection';

const fakeFm = { cellW: 8, cellH: 16, baseline: 12, fontCss: '14px monospace', dpr: 1 };

describe('Selection', () => {
  it('renders one rect for single-row selection', () => {
    const { container } = render(
      <Selection
        selection={{ anchor: { col: 2, row: 1 }, head: { col: 7, row: 1 } }}
        cols={80}
        fm={fakeFm}
      />,
    );
    expect(container.querySelectorAll('[data-rect]').length).toBe(1);
  });

  it('renders two rects for 2-row selection (head/tail only, no body)', () => {
    const { container } = render(
      <Selection
        selection={{ anchor: { col: 2, row: 1 }, head: { col: 5, row: 2 } }}
        cols={80}
        fm={fakeFm}
      />,
    );
    expect(container.querySelectorAll('[data-rect]').length).toBe(2);
  });

  it('renders three rects for 3+ row selection (head/body/tail)', () => {
    const { container } = render(
      <Selection
        selection={{ anchor: { col: 2, row: 1 }, head: { col: 5, row: 4 } }}
        cols={80}
        fm={fakeFm}
      />,
    );
    expect(container.querySelectorAll('[data-rect]').length).toBe(3);
  });

  it('normalizes reversed selection (head before anchor)', () => {
    const { container } = render(
      <Selection
        selection={{ anchor: { col: 7, row: 3 }, head: { col: 2, row: 1 } }}
        cols={80}
        fm={fakeFm}
      />,
    );
    expect(container.querySelectorAll('[data-rect]').length).toBe(3);
  });

  it('renders nothing for a zero-length (caret) selection', () => {
    const { container } = render(
      <Selection
        selection={{ anchor: { col: 5, row: 2 }, head: { col: 5, row: 2 } }}
        cols={80}
        fm={fakeFm}
      />,
    );
    expect(container.querySelectorAll('[data-rect]').length).toBe(0);
  });

  it('single-row rect spans (end.col - start.col) cells', () => {
    const { container } = render(
      <Selection
        selection={{ anchor: { col: 2, row: 1 }, head: { col: 7, row: 1 } }}
        cols={80}
        fm={fakeFm}
      />,
    );
    const rect = container.querySelector('[data-rect]') as HTMLElement;
    expect(rect.style.left).toBe('16px');
    expect(rect.style.top).toBe('16px');
    expect(rect.style.width).toBe('40px');
    expect(rect.style.height).toBe('16px');
  });
});
