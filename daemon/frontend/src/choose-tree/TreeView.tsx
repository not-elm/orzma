import type { SessionId } from '../layout/types';
import { flattenVisibleRows, type VisibleRow } from './flattenVisibleRows';
import type { TreeCursor } from './treeReducer';
import type { SessionTreeNode } from './types';

interface TreeViewProps {
  tree: SessionTreeNode[];
  expanded: ReadonlySet<SessionId>;
  cursor: TreeCursor;
  onRowClick: (cursor: TreeCursor) => void;
}

function rowKey(row: VisibleRow): string {
  if (row.kind === 'session') return `session:${row.sessionId}`;
  return `window:${row.sessionId}:${row.windowId}`;
}

function rowMatchesCursor(row: VisibleRow, cursor: TreeCursor): boolean {
  if (cursor.kind === 'session')
    return row.kind === 'session' && row.sessionId === cursor.sessionId;
  return (
    row.kind === 'window' && row.sessionId === cursor.sessionId && row.windowId === cursor.windowId
  );
}

/**
 * Computes the DOM id for the row that the cursor currently points at,
 * or `undefined` when no matching row is visible.
 */
export function activeRowKey(
  tree: SessionTreeNode[],
  expanded: ReadonlySet<SessionId>,
  cursor: TreeCursor,
): string | undefined {
  const rows = flattenVisibleRows(tree, expanded);
  const activeRow = rows.find((r) => rowMatchesCursor(r, cursor));
  return activeRow ? rowKey(activeRow) : undefined;
}

/**
 * Renders the visible rows as a flat list of `role="treeitem"`s under a
 * single `role="tree"`. Row identity comes from `session:${sid}` /
 * `window:${sid}:${wid}` so React reconciliation stays stable across
 * expand / collapse and tree reloads.
 */
export function TreeView({ tree, expanded, cursor, onRowClick }: TreeViewProps) {
  const rows = flattenVisibleRows(tree, expanded);
  return (
    <div role="tree" className="font-mono text-sm">
      {rows.map((row) => {
        const id = rowKey(row);
        const selected = rowMatchesCursor(row, cursor);
        if (row.kind === 'session') {
          const sessionCursor: TreeCursor = { kind: 'session', sessionId: row.sessionId };
          return (
            <div
              key={id}
              id={id}
              role="treeitem"
              aria-level={1}
              aria-expanded={row.expanded}
              aria-selected={selected}
              tabIndex={-1}
              onClick={() => onRowClick(sessionCursor)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') onRowClick(sessionCursor);
              }}
              className={`flex cursor-pointer items-center gap-2 px-2 py-0.5 ${
                selected ? 'bg-tmux-pane-active text-background' : ''
              }`}
            >
              <span aria-hidden="true">{row.expanded ? '▼' : '▶'}</span>
              <span className="font-semibold">{row.name}</span>
              <span className="text-muted-foreground">({row.windowCount})</span>
            </div>
          );
        }
        const windowCursor: TreeCursor = {
          kind: 'window',
          sessionId: row.sessionId,
          windowId: row.windowId,
        };
        return (
          <div
            key={id}
            id={id}
            role="treeitem"
            aria-level={2}
            aria-selected={selected}
            tabIndex={-1}
            onClick={() => onRowClick(windowCursor)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') onRowClick(windowCursor);
            }}
            className={`flex cursor-pointer items-center gap-2 py-0.5 pl-8 pr-2 ${
              selected ? 'bg-tmux-pane-active text-background' : ''
            }`}
          >
            <span className="w-6 text-muted-foreground">{row.index}:</span>
            <span>{row.name}</span>
          </div>
        );
      })}
    </div>
  );
}
