import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { SessionSegment } from './SessionSegment';

describe('SessionSegment', () => {
  it('renders Loading… for loading state', () => {
    render(<SessionSegment state={{ status: 'loading' }} />);
    expect(screen.getByText('Loading…')).toBeInTheDocument();
  });

  it('renders the session name when ready', () => {
    render(<SessionSegment state={{ status: 'ready', name: 'ozmux' }} />);
    expect(screen.getByText('ozmux')).toBeInTheDocument();
  });

  it('renders a destructive message when gone', () => {
    render(<SessionSegment state={{ status: 'gone', reason: 'session_not_found' }} />);
    expect(screen.getByText(/Session is gone/)).toHaveClass('text-destructive');
  });
});
