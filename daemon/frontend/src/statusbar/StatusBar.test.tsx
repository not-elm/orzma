import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { StatusBar } from './StatusBar';
import type { SessionViewState } from './useSessionView';

function liveState(): Extract<SessionViewState, { status: 'live' }> {
  return {
    status: 'live',
    view: {
      id: 'sid-0',
      name: 'ozmux',
      active_window: 'wid-0',
      windows: [{ id: 'wid-0', name: 'main', index: 0 }],
    },
  };
}

describe('StatusBar', () => {
  it('shows Loading… while connecting with no view', () => {
    render(
      <StatusBar
        sessionState={{ status: 'connecting', view: null }}
        windowReconnecting={false}
        onSelectWindow={vi.fn()}
      />,
    );
    expect(screen.getByText('Loading…')).toBeInTheDocument();
  });

  it('renders session name + windows when live', () => {
    render(
      <StatusBar sessionState={liveState()} windowReconnecting={false} onSelectWindow={vi.fn()} />,
    );
    expect(screen.getByText('ozmux')).toBeInTheDocument();
    expect(screen.getByRole('button', { current: 'page' })).toHaveTextContent('0:main*');
  });

  it('shows the pill when sessionReconnecting', () => {
    render(
      <StatusBar
        sessionState={{ status: 'reconnecting', view: liveState().view, retryInSec: 0 }}
        windowReconnecting={false}
        onSelectWindow={vi.fn()}
      />,
    );
    expect(screen.getByText('Reconnecting…')).toBeInTheDocument();
  });

  it('shows the pill when windowReconnecting', () => {
    render(
      <StatusBar sessionState={liveState()} windowReconnecting={true} onSelectWindow={vi.fn()} />,
    );
    expect(screen.getByText('Reconnecting…')).toBeInTheDocument();
  });
});
