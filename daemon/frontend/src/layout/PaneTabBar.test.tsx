import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { PaneTabBar } from './PaneTabBar';
import type { PaneView } from './types';

const pane: PaneView = {
  id: 'pid-1',
  active_activity: 'aid-1',
  activities: [
    { id: 'aid-1', kind: 'terminal', title: 'zsh' },
    { id: 'aid-2', kind: 'terminal', title: 'vim CLAUDE.md' },
  ],
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('PaneTabBar', () => {
  it('renders one tab per activity and marks the active one', () => {
    render(<PaneTabBar windowId="wid-1" pane={pane} isActive onActivate={() => {}} />);
    const tabs = screen.getAllByRole('tab');
    expect(tabs).toHaveLength(2);
    expect(screen.getByText('zsh')).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByText('vim CLAUDE.md')).toHaveAttribute('aria-selected', 'false');
  });

  it('POSTs the activate endpoint when an inactive tab is clicked', () => {
    const fetchMock = vi.fn(() => Promise.resolve(new Response()));
    vi.stubGlobal('fetch', fetchMock);
    render(<PaneTabBar windowId="wid-1" pane={pane} isActive onActivate={() => {}} />);
    fireEvent.pointerDown(screen.getByText('vim CLAUDE.md'));
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-1/activities/aid-2/activate', {
      method: 'POST',
    });
  });

  it('activates the pane before the activity when the pane is inactive', () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(new Response())),
    );
    const onActivate = vi.fn();
    render(<PaneTabBar windowId="wid-1" pane={pane} isActive={false} onActivate={onActivate} />);
    fireEvent.pointerDown(screen.getByText('vim CLAUDE.md'));
    expect(onActivate).toHaveBeenCalledOnce();
  });
});
