import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { IME } from './IME';

const fakeFm = {
  cellW: 8,
  cellH: 16,
  baseline: 12,
  fontCss: '14px monospace',
  dpr: 1,
  letterSpacing: 0,
};

describe('IME', () => {
  it('renders preedit text at cursor cell coords', () => {
    const { container } = render(
      <IME
        preedit="こんにちは"
        cursor={{ x: 3, y: 5, shape: 'block', blinking: false, visible: true }}
        fm={fakeFm}
      />,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.style.left).toBe('24px');
    expect(div.style.top).toBe('80px');
    expect(div.textContent).toBe('こんにちは');
  });
});
