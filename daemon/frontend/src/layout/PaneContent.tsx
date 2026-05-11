import { Terminal } from '../terminal/Terminal';
import { PanePlaceholder } from './PanePlaceholder';
import type { PaneView } from './types';

export function PaneContent({ pane }: { pane: PaneView }) {
  const activity = pane.activities.find((a) => a.id === pane.active_activity);
  if (!activity) return <PanePlaceholder paneId={pane.id} />;
  if (activity.kind === 'extension') {
    if (!activity.iframe_url) return <PanePlaceholder paneId={pane.id} />;
    return (
      <iframe
        src={activity.iframe_url}
        title={`extension-${activity.id}`}
        className="h-full w-full border-0"
      />
    );
  }
  return <Terminal key={activity.id} activityId={activity.id} />;
}
