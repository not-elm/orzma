import type { SessionId, WindowId } from '../layout/types';
import type { SessionTreeNode } from './types';

export type VisibleRow =
  | { kind: 'session'; sessionId: SessionId; name: string; expanded: boolean; windowCount: number }
  | { kind: 'window'; sessionId: SessionId; windowId: WindowId; name: string; index: number };

/**
 * Builds the linear list of rows that the picker actually renders, in
 * top-to-bottom order. Pure function: no React state, no DOM.
 */
export function flattenVisibleRows(
  tree: SessionTreeNode[],
  expanded: ReadonlySet<SessionId>,
): VisibleRow[] {
  const rows: VisibleRow[] = [];
  for (const session of tree) {
    const isExpanded = expanded.has(session.id);
    rows.push({
      kind: 'session',
      sessionId: session.id,
      name: session.name,
      expanded: isExpanded,
      windowCount: session.windows.length,
    });
    if (isExpanded) {
      for (const w of session.windows) {
        rows.push({
          kind: 'window',
          sessionId: session.id,
          windowId: w.id,
          name: w.name,
          index: w.index,
        });
      }
    }
  }
  return rows;
}
