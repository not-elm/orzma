import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { SessionWindowEntry } from './types';
import { WindowListSegment } from './WindowListSegment';

const WINDOWS: SessionWindowEntry[] = [
  { id: 'wid-0', name: 'main', index: 0 },
  { id: 'wid-1', name: 'logs', index: 1 },
];

describe('WindowListSegment', () => {
  it('renders each window with index:name', () => {
    render(<WindowListSegment windows={WINDOWS} activeWindowId="wid-0" onSelect={vi.fn()} />);
    expect(screen.getByText(/^0:main/)).toBeInTheDocument();
    expect(screen.getByText('1:logs')).toBeInTheDocument();
  });

  it('marks the active chip with aria-current and trailing *', () => {
    render(<WindowListSegment windows={WINDOWS} activeWindowId="wid-1" onSelect={vi.fn()} />);
    const active = screen.getByRole('button', { current: 'page' });
    expect(active).toHaveTextContent('1:logs*');
  });

  it('calls onSelect with the clicked window id', () => {
    const onSelect = vi.fn();
    render(<WindowListSegment windows={WINDOWS} activeWindowId="wid-0" onSelect={onSelect} />);
    fireEvent.click(screen.getByText('1:logs'));
    expect(onSelect).toHaveBeenCalledWith('wid-1');
  });

  it('calls scrollIntoView on the active chip when activeWindowId changes', () => {
    const scrollSpy = vi.fn();
    Element.prototype.scrollIntoView = scrollSpy as unknown as typeof Element.prototype.scrollIntoView;
    const { rerender } = render(
      <WindowListSegment windows={WINDOWS} activeWindowId="wid-0" onSelect={vi.fn()} />,
    );
    scrollSpy.mockClear();
    rerender(<WindowListSegment windows={WINDOWS} activeWindowId="wid-1" onSelect={vi.fn()} />);
    expect(scrollSpy).toHaveBeenCalled();
  });
});
