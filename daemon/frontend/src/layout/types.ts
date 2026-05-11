// Mirrors backend `WindowLayoutNode` + WindowView JSON.
// Contract reference: docs/superpowers/specs/2026-05-10-frontend-pane-layout-design.md §3

export type CellId = string;
export type PaneId = string;
export type WindowId = string;
export type ActivityId = string;
export type SessionId = string;

export type SplitOrientation = 'horizontal' | 'vertical';

export type WindowLayoutNode =
  | { type: 'root'; cell_id: CellId; child: WindowLayoutNode }
  | {
      type: 'split';
      cell_id: CellId;
      orientation: SplitOrientation;
      split_ratio: number;
      lhs: WindowLayoutNode;
      rhs: WindowLayoutNode;
    }
  | { type: 'pane'; cell_id: CellId; pane_id: PaneId };

export interface PaneView {
  id: PaneId;
  active_activity: ActivityId;
  activities: Array<{ id: ActivityId; kind: 'terminal' | 'extension'; iframe_url?: string }>;
}

export interface WindowView {
  id: WindowId;
  name: string;
  root_cell: CellId;
  active_pane: PaneId;
  panes: PaneView[];
  layout_schema_version: 1;
  layout: WindowLayoutNode;
}
