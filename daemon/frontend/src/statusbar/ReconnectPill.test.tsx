import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ReconnectPill } from './ReconnectPill';

describe('ReconnectPill', () => {
  it('renders nothing when visible is false', () => {
    const { container } = render(<ReconnectPill visible={false} />);
    expect(container.firstChild).toBeNull();
  });

  it('renders Reconnecting… with live-region semantics when visible', () => {
    render(<ReconnectPill visible={true} />);
    const pill = screen.getByText('Reconnecting…');
    expect(pill).toHaveAttribute('role', 'status');
    expect(pill).toHaveAttribute('aria-live', 'polite');
  });
});
