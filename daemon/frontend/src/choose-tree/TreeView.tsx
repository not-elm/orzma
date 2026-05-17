import type { KeyboardEvent } from 'react';
import type { VisibleRow } from './flattenVisibleRows';
import { rowMatches, type TreeCursor } from './treeReducer';

interface TreeViewProps {
  rows: VisibleRow[];
  cursor: TreeCursor;
  onRowClick: (cursor: TreeCursor) => void;
}

type SessionVisibleRow = Extract<VisibleRow, { kind: 'session' }>;
type WindowVisibleRow = Extract<VisibleRow, { kind: 'window' }>;

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

function activateOnEnterOrSpace(select: () => void) {
  return (e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') select();
  };
}

interface SessionRowProps {
  row: SessionVisibleRow;
  selected: boolean;
  onSelect: (cursor: TreeCursor) => void;
}

function SessionRow({ row, selected, onSelect }: SessionRowProps) {
  const select = () => onSelect({ kind: 'session', sessionId: row.sessionId });
  return (
    <div
      id={rowKey(row)}
      role="treeitem"
      aria-level={1}
      aria-expanded={row.expanded}
      aria-selected={selected}
      tabIndex={-1}
      onClick={select}
      onKeyDown={activateOnEnterOrSpace(select)}
      className={`${ROW_BASE} gap-2 py-1.5 pr-3 pl-7 ${selected ? ROW_SELECTED : ROW_HOVER}`}
    >
      {selected && <BlinkingCursor />}
      <span
        aria-hidden="true"
        className={`w-4 text-xs ${selected ? 'text-primary' : 'text-muted-foreground'}`}
      >
        {row.expanded ? '▼' : '▶'}
      </span>
      <span className={`font-semibold tracking-wide ${selected ? 'text-info' : 'text-foreground'}`}>
        {row.name}
      </span>
      <span className="text-muted-foreground text-xs">
        {row.windowCount} {row.windowCount === 1 ? 'window' : 'windows'}
      </span>
    </div>
  );
}

interface WindowRowProps {
  row: WindowVisibleRow;
  selected: boolean;
  onSelect: (cursor: TreeCursor) => void;
}

function WindowRow({ row, selected, onSelect }: WindowRowProps) {
  const select = () =>
    onSelect({ kind: 'window', sessionId: row.sessionId, windowId: row.windowId });
  return (
    <div
      id={rowKey(row)}
      role="treeitem"
      aria-level={2}
      aria-selected={selected}
      tabIndex={-1}
      onClick={select}
      onKeyDown={activateOnEnterOrSpace(select)}
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
        const selected = rowMatches(row, cursor);
        const key = rowKey(row);
        return row.kind === 'session' ? (
          <SessionRow key={key} row={row} selected={selected} onSelect={onRowClick} />
        ) : (
          <WindowRow key={key} row={row} selected={selected} onSelect={onRowClick} />
        );
      })}
    </div>
  );
}
