import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { LayoutView } from './LayoutView';
import type { DefaultWindowState, WindowView } from './types';
import type { LayoutState } from './useWindowLayout';

const WID = 'wid-test';

function fakeView(overrides: Partial<WindowView> = {}): WindowView {
  return {
    id: WID,
    name: 'main',
    root_cell: 'cid-root',
    active_pane: 'pid-1',
    panes: [
      {
        id: 'pid-1',
        active_activity: 'aid-1',
        activities: [{ id: 'aid-1', kind: 'terminal', title: 'zsh' }],
      },
    ],
    layout: {
      type: 'root',
      cell_id: 'cid-root',
      child: { type: 'pane', cell_id: 'cid-pane-1', pane_id: 'pid-1' },
    },
    ...overrides,
  };
}

describe('LayoutView', () => {
  it('shows loading when window discovery is loading', () => {
    const def: DefaultWindowState = { status: 'loading' };
    const layout: LayoutState = { status: 'connecting', view: null };
    render(<LayoutView windowState={def} layoutState={layout} />);
    expect(screen.getByText(/loading/i)).toBeInTheDocument();
  });

  it('shows error when window discovery fails', () => {
    const def: DefaultWindowState = { status: 'error', message: 'boom' };
    const layout: LayoutState = { status: 'connecting', view: null };
    render(<LayoutView windowState={def} layoutState={layout} />);
    expect(screen.getByText(/failed to discover window/i)).toBeInTheDocument();
    expect(screen.getByText(/boom/)).toBeInTheDocument();
  });

  it('shows gone when the window is gone', () => {
    const def: DefaultWindowState = { status: 'ready', windowId: WID };
    const layout: LayoutState = { status: 'gone', reason: 'window_closed' };
    render(<LayoutView windowState={def} layoutState={layout} />);
    expect(screen.getByText(/window is gone/i)).toBeInTheDocument();
  });

  it('renders panes when layout is live', () => {
    const def: DefaultWindowState = { status: 'ready', windowId: WID };
    const layout: LayoutState = { status: 'live', view: fakeView() };
    const { container } = render(<LayoutView windowState={def} layoutState={layout} />);
    // pane outline element is rendered
    expect(container.querySelector('[data-active="true"]')).not.toBeNull();
  });
});
