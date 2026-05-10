import { Terminal } from '../terminal/Terminal';
import { PanePlaceholder } from './PanePlaceholder';
import { UnknownLayoutNode } from './UnknownLayoutNode';
import type { WindowLayoutNode, WindowView } from './types';

interface Props {
  node: WindowLayoutNode;
  view: WindowView;
}

export function CellNode({ node, view }: Props) {
  if (node.type === 'root') {
    return <CellNode key={node.cell_id} node={node.child} view={view} />;
  }
  if (node.type === 'split') {
    const direction = node.orientation === 'horizontal' ? 'row' : 'column';
    return (
      <div
        key={node.cell_id}
        style={{ display: 'flex', flexDirection: direction, height: '100%', width: '100%' }}
      >
        <div style={{ flex: node.split_ratio }}>
          <CellNode node={node.lhs} view={view} />
        </div>
        <div style={{ flex: 1 - node.split_ratio }}>
          <CellNode node={node.rhs} view={view} />
        </div>
      </div>
    );
  }
  if (node.type === 'pane') {
    if (node.pane_id === view.active_pane) {
      const pane = view.panes.find((p) => p.id === node.pane_id);
      const activityId = pane?.active_activity;
      if (!activityId) return <PanePlaceholder paneId={node.pane_id} />;
      return <Terminal activityId={activityId} />;
    }
    return <PanePlaceholder paneId={node.pane_id} />;
  }
  return <UnknownLayoutNode type={(node as { type: string }).type} />;
}
