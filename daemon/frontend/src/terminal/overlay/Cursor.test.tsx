import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Cursor } from './Cursor';

const fakeFm = {
  cellW: 8,
  cellH: 16,
  baseline: 12,
  fontCss: '14px monospace',
  dpr: 1,
  letterSpacing: 0,
};

describe('Cursor', () => {
  it('renders a div positioned at cursor x/y with full cell size for block shape', () => {
    const { container } = render(
      <Cursor
        cursor={{ x: 3, y: 5, shape: 'block', blinking: true, visible: true }}
        isActive={true}
        fm={fakeFm}
      />,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.style.left).toBe('24px');
    expect(div.style.top).toBe('80px');
    expect(div.style.width).toBe('8px');
    expect(div.style.height).toBe('16px');
  });

  it('applies animate-cursor-blink class when active and blinking', () => {
    const { container } = render(
      <Cursor
        cursor={{ x: 0, y: 0, shape: 'block', blinking: true, visible: true }}
        isActive={true}
        fm={fakeFm}
      />,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.className).toContain('animate-cursor-blink');
  });

  it('does not blink when blinking=false (steady cursor)', () => {
    const { container } = render(
      <Cursor
        cursor={{ x: 0, y: 0, shape: 'block', blinking: false, visible: true }}
        isActive={true}
        fm={fakeFm}
      />,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.className).not.toContain('animate-cursor-blink');
  });

  it('does not blink when not active', () => {
    const { container } = render(
      <Cursor
        cursor={{ x: 0, y: 0, shape: 'block', blinking: true, visible: true }}
        isActive={false}
        fm={fakeFm}
      />,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.className).not.toContain('animate-cursor-blink');
  });

  it('reduces height for underline shape', () => {
    const { container } = render(
      <Cursor
        cursor={{ x: 0, y: 0, shape: 'underline', blinking: false, visible: true }}
        isActive={true}
        fm={fakeFm}
      />,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.style.height).toBe('2px');
  });

  it('reduces width for bar shape', () => {
    const { container } = render(
      <Cursor
        cursor={{ x: 0, y: 0, shape: 'bar', blinking: false, visible: true }}
        isActive={true}
        fm={fakeFm}
      />,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.style.width).toBe('2px');
  });

  it('renders nothing when cursor.visible is false', () => {
    const { container } = render(
      <Cursor
        cursor={{ x: 0, y: 0, shape: 'block', blinking: false, visible: false }}
        isActive={true}
        fm={fakeFm}
      />,
    );
    expect(container.firstChild).toBeNull();
  });
});
