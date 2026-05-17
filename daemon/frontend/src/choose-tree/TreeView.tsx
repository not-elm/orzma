import type { VisibleRow } from './flattenVisibleRows';
import { rowMatches, type TreeCursor } from './treeReducer';

interface TreeViewProps {
  rows: VisibleRow[];
  cursor: TreeCursor;
  onRowClick: (cursor: TreeCursor) => void;
}

function rowKey(row: VisibleRow): string {
  if (row.kind === 'session') return `session:${row.sessionId}`;
  return `window:${row.sessionId}:${row.windowId}`;
}

/**
 * Returns the DOM id of the visible row the cursor points at, or
 * `undefined` when no matching row is visible (e.g. cursor sits under a
 * collapsed session).
 */
export function activeRowKey(rows: VisibleRow[], cursor: TreeCursor): string | undefined {
  const activeRow = rows.find((r) => rowMatches(r, cursor));
  return activeRow ? rowKey(activeRow) : undefined;
}

const ROW_BASE = 'group relative flex cursor-pointer items-center font-mono transition-colors';
const ROW_HOVER = 'hover:bg-muted/40';
const ROW_SELECTED = 'bg-primary text-primary-foreground';

/**
 * Renders the visible rows as a flat list of `role="treeitem"`s under a
 * single `role="tree"`. Row identity comes from `session:${sid}` /
 * `window:${sid}:${wid}` so React reconciliation stays stable across
 * expand / collapse and tree reloads.
 */
export function TreeView({ rows, cursor, onRowClick }: TreeViewProps) {
  return (
    <div role="tree" className="py-1 text-sm">
      {rows.map((row) => {
        const id = rowKey(row);
        const selected = rowMatches(row, cursor);
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
              className={`${ROW_BASE} gap-2 px-3 py-1.5 ${selected ? ROW_SELECTED : ROW_HOVER}`}
            >
              {selected && (
                <span
                  aria-hidden="true"
                  className="absolute left-0 top-0 bottom-0 w-1 bg-primary-foreground"
                />
              )}
              <span
                aria-hidden="true"
                className={`w-4 text-xs ${selected ? 'opacity-80' : 'text-muted-foreground'}`}
              >
                {row.expanded ? '▼' : '▶'}
              </span>
              <span className="font-semibold tracking-wide">{row.name}</span>
              <span className={`text-xs ${selected ? 'opacity-70' : 'text-muted-foreground'}`}>
                {row.windowCount} {row.windowCount === 1 ? 'window' : 'windows'}
              </span>
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
            className={`${ROW_BASE} gap-3 py-1 pl-10 pr-3 ${selected ? ROW_SELECTED : ROW_HOVER}`}
          >
            {selected && (
              <span
                aria-hidden="true"
                className="absolute left-0 top-0 bottom-0 w-1 bg-primary-foreground"
              />
            )}
            <span
              aria-hidden="true"
              className={`w-3 text-center text-xs ${
                row.isActive ? (selected ? 'text-primary-foreground' : 'text-warning') : 'opacity-0'
              }`}
              title={row.isActive ? 'Active window in its session' : undefined}
            >
              ★
            </span>
            <span className={`tabular-nums ${selected ? 'opacity-70' : 'text-muted-foreground'}`}>
              {row.index}
            </span>
            <span className={selected ? 'font-medium' : ''}>{row.name}</span>
          </div>
        );
      })}
    </div>
  );
}
