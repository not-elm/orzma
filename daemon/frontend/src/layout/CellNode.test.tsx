import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { CellNode } from './CellNode';
import type { WindowLayoutNode, WindowView } from './types';

const baseView: WindowView = {
  id: 'wid',
  name: 'main',
  root_cell: 'cid-root',
  active_pane: 'pid-1',
  panes: [
    { id: 'pid-1', active_activity: 'aid-1', activities: [{ id: 'aid-1', kind: 'terminal' }] },
  ],
  layout_schema_version: 1,
  layout: {
    type: 'root',
    cell_id: 'cid-root',
    child: { type: 'pane', cell_id: 'cid-pane', pane_id: 'pid-1' },
  },
};

describe('<CellNode>', () => {
  it('renders pane placeholder for non-active pane', () => {
    const node: WindowLayoutNode = { type: 'pane', cell_id: 'cid-x', pane_id: 'pid-other' };
    const view: WindowView = {
      ...baseView,
      panes: [
        ...baseView.panes,
        { id: 'pid-other', active_activity: 'aid-2', activities: [] },
      ],
    };
    const { getByText } = render(<CellNode node={node} view={view} />);
    expect(getByText(/pid-other/)).toBeInTheDocument();
  });

  it('renders flex row for horizontal split', () => {
    const node: WindowLayoutNode = {
      type: 'split',
      cell_id: 'cid-split',
      orientation: 'horizontal',
      split_ratio: 0.7,
      lhs: { type: 'pane', cell_id: 'cid-l', pane_id: 'pid-a' },
      rhs: { type: 'pane', cell_id: 'cid-r', pane_id: 'pid-b' },
    };
    const view: WindowView = {
      ...baseView,
      active_pane: 'pid-none',
      panes: [
        { id: 'pid-a', active_activity: 'aid-a', activities: [] },
        { id: 'pid-b', active_activity: 'aid-b', activities: [] },
      ],
    };
    const { container } = render(<CellNode node={node} view={view} />);
    const split = container.firstChild as HTMLElement;
    expect(split.style.flexDirection).toBe('row');
  });

  it('renders flex column for vertical split', () => {
    const node: WindowLayoutNode = {
      type: 'split',
      cell_id: 'cid-split',
      orientation: 'vertical',
      split_ratio: 0.5,
      lhs: { type: 'pane', cell_id: 'cid-l', pane_id: 'pid-a' },
      rhs: { type: 'pane', cell_id: 'cid-r', pane_id: 'pid-b' },
    };
    const view: WindowView = {
      ...baseView,
      active_pane: 'pid-none',
      panes: [
        { id: 'pid-a', active_activity: 'aid-a', activities: [] },
        { id: 'pid-b', active_activity: 'aid-b', activities: [] },
      ],
    };
    const { container } = render(<CellNode node={node} view={view} />);
    const split = container.firstChild as HTMLElement;
    expect(split.style.flexDirection).toBe('column');
  });

  it('renders UnknownLayoutNode for unrecognized type', () => {
    const bogus = { type: 'unknown_type', cell_id: 'x' } as unknown as WindowLayoutNode;
    const { getByText } = render(<CellNode node={bogus} view={baseView} />);
    expect(getByText(/Unknown layout node type/)).toBeInTheDocument();
  });

  it('renders an iframe (not Terminal) for an active extension pane', () => {
    const node: WindowLayoutNode = {
      type: 'pane',
      cell_id: 'cid-ext',
      pane_id: 'pid-ext',
    };
    const view: WindowView = {
      ...baseView,
      active_pane: 'pid-ext',
      panes: [
        {
          id: 'pid-ext',
          active_activity: 'aid-ext',
          activities: [
            {
              id: 'aid-ext',
              kind: 'extension',
              iframe_url: '/activities/aid-ext/iframe/index.html',
            },
          ],
        },
      ],
    };
    const { container } = render(<CellNode node={node} view={view} />);
    const iframe = container.querySelector('iframe');
    expect(iframe).not.toBeNull();
    expect(iframe?.getAttribute('src')).toBe('/activities/aid-ext/iframe/index.html');
  });

  it('falls back to PanePlaceholder for an extension activity without iframe_url', () => {
    const node: WindowLayoutNode = {
      type: 'pane',
      cell_id: 'cid-ext',
      pane_id: 'pid-ext',
    };
    const view: WindowView = {
      ...baseView,
      active_pane: 'pid-ext',
      panes: [
        {
          id: 'pid-ext',
          active_activity: 'aid-ext',
          activities: [{ id: 'aid-ext', kind: 'extension' }],
        },
      ],
    };
    const { getByText, container } = render(<CellNode node={node} view={view} />);
    expect(getByText(/pid-ext/)).toBeInTheDocument();
    expect(container.querySelector('iframe')).toBeNull();
  });
});
