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
const ROW_SELECTED = 'bg-primary/15';

function BlinkingCursor() {
  return (
    <span
      aria-hidden="true"
      className="-translate-y-1/2 absolute top-1/2 left-2 animate-cursor-blink text-primary"
    >
      ▌
    </span>
  );
}

/**
 * Renders the visible rows as a flat list of `role="treeitem"`s under a
 * single `role="tree"`. Row identity comes from `session:${sid}` /
 * `window:${sid}:${wid}` so React reconciliation stays stable across
 * expand / collapse and tree reloads.
 *
 * Selection follows a Neovim-style pattern: a soft `bg-primary/15` tint
 * plus a blinking `▌` block cursor pinned to the row's left edge.
 * Selected names switch to `text-info` (Tokyo Night cyan) for contrast
 * against the tinted background.
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
              className={`${ROW_BASE} gap-2 py-1.5 pr-3 pl-7 ${selected ? ROW_SELECTED : ROW_HOVER}`}
            >
              {selected && <BlinkingCursor />}
              <span
                aria-hidden="true"
                className={`w-4 text-xs ${selected ? 'text-primary' : 'text-muted-foreground'}`}
              >
                {row.expanded ? '▼' : '▶'}
              </span>
              <span
                className={`font-semibold tracking-wide ${selected ? 'text-info' : 'text-foreground'}`}
              >
                {row.name}
              </span>
              <span className="text-muted-foreground text-xs">
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
            className={`${ROW_BASE} gap-3 py-1 pr-3 pl-10 ${selected ? ROW_SELECTED : ROW_HOVER}`}
          >
            {selected && <BlinkingCursor />}
            <span
              aria-hidden="true"
              className={`w-3 text-center text-xs ${row.isActive ? 'text-warning' : 'opacity-0'}`}
              title={row.isActive ? 'Active window in its session' : undefined}
            >
              ★
            </span>
            <span className="text-muted-foreground tabular-nums">{row.index}</span>
            <span className={selected ? 'text-info' : 'text-foreground'}>{row.name}</span>
          </div>
        );
      })}
    </div>
  );
}
