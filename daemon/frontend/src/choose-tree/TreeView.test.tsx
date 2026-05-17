import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { flattenVisibleRows } from './flattenVisibleRows';
import { TreeView } from './TreeView';
import type { SessionTreeNode } from './types';

const tree: SessionTreeNode[] = [
  {
    id: 'sid-a',
    name: 'work',
    active_window: 'wid-a0',
    windows: [{ id: 'wid-a0', name: 'build', index: 0 }],
  },
];

describe('TreeView', () => {
  it('renders session and window rows with ARIA roles', () => {
    const rows = flattenVisibleRows(tree, new Set(['sid-a']));
    render(
      <TreeView
        rows={rows}
        cursor={{ kind: 'window', sessionId: 'sid-a', windowId: 'wid-a0' }}
        onRowClick={() => {}}
      />,
    );
    expect(screen.getByRole('tree')).toBeInTheDocument();
    const items = screen.getAllByRole('treeitem');
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveAttribute('aria-expanded', 'true');
    expect(items[1]).toHaveAttribute('aria-selected', 'true');
  });

  it('marks collapsed sessions with aria-expanded=false', () => {
    const rows = flattenVisibleRows(tree, new Set());
    render(
      <TreeView
        rows={rows}
        cursor={{ kind: 'session', sessionId: 'sid-a' }}
        onRowClick={() => {}}
      />,
    );
    const items = screen.getAllByRole('treeitem');
    expect(items).toHaveLength(1);
    expect(items[0]).toHaveAttribute('aria-expanded', 'false');
  });
});
