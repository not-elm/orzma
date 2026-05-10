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
    const pane = view.panes.find((p) => p.id === node.pane_id);
    const activity = pane?.activities.find((a) => a.id === pane.active_activity);
    const isActive = node.pane_id === view.active_pane;
    const borderClass = isActive
      ? 'border-2 border-tmux-pane-active'
      : 'border border-tmux-pane-border';
    if (!activity) {
      return (
        <div
          data-active={isActive}
          className={`relative h-full w-full ${borderClass}`}
        >
          <PanePlaceholder paneId={node.pane_id} />
        </div>
      );
    }
    if (activity.kind === 'extension') {
      if (!activity.iframe_url) {
        return (
          <div
            data-active={isActive}
            className={`relative h-full w-full ${borderClass}`}
          >
            <PanePlaceholder paneId={node.pane_id} />
          </div>
        );
      }
      return (
        <div
          data-active={isActive}
          className={`relative h-full w-full ${borderClass}`}
        >
          <iframe
            src={activity.iframe_url}
            title={`extension-${activity.id}`}
            style={{ width: '100%', height: '100%', border: 0 }}
          />
        </div>
      );
    }
    return (
      <div
        data-active={isActive}
        className={`relative h-full w-full ${borderClass}`}
      >
        <Terminal key={activity.id} activityId={activity.id} />
      </div>
    );
  }
  return <UnknownLayoutNode type={(node as { type: string }).type} />;
}
