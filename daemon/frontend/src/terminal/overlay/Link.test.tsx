import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Link } from './Link';

const fakeFm = { cellW: 8, cellH: 16, baseline: 12, fontCss: '14px monospace', dpr: 1 };

describe('Link', () => {
  it('renders an underline rect for the hover range', () => {
    const { container } = render(
      <Link
        hover={{ rangeStart: 5, rangeEnd: 10, row: 2, uri: 'https://example.com' }}
        fm={fakeFm}
      />,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.style.left).toBe('40px');
    expect(div.style.top).toBe('32px');
    expect(div.style.width).toBe('40px');
    expect(div.style.height).toBe('16px');
    expect(div.getAttribute('data-uri')).toBe('https://example.com');
  });
});
