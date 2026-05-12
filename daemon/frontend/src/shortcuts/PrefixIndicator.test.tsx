import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { PrefixIndicator } from './PrefixIndicator';

describe('PrefixIndicator', () => {
  it('renders nothing when not armed', () => {
    const { container } = render(<PrefixIndicator armed={false} />);
    expect(container.firstChild).toBeNull();
  });

  it('renders ^B when armed', () => {
    render(<PrefixIndicator armed={true} />);
    expect(screen.getByText('^B')).toBeInTheDocument();
  });

  it('uses role=status with polite aria-live', () => {
    render(<PrefixIndicator armed={true} />);
    const el = screen.getByRole('status');
    expect(el).toHaveAttribute('aria-live', 'polite');
  });
});
