import { BrowserActivity } from '../browser';
import { ExtensionActivity } from '../extension';
import { Terminal } from '../terminal/Terminal';
import { ClickShield } from './ClickShield';
import { PanePlaceholder } from './PanePlaceholder';
import { PaneTabBar } from './PaneTabBar';
import type { PaneView } from './types';

interface PaneContentProps {
  windowId: string;
  pane: PaneView;
  isActive: boolean;
  onActivate: () => void;
  replay?: string;
  recordPerf?: boolean;
}

export function PaneContent({
  windowId,
  pane,
  isActive,
  onActivate,
  replay,
  recordPerf,
}: PaneContentProps) {
  return (
    <div className="flex h-full w-full flex-col">
      <PaneTabBar windowId={windowId} pane={pane} isActive={isActive} onActivate={onActivate} />
      <div className="relative min-h-0 flex-1">
        <PaneBody
          windowId={windowId}
          pane={pane}
          isActive={isActive}
          onActivate={onActivate}
          replay={replay}
          recordPerf={recordPerf}
        />
      </div>
    </div>
  );
}

function PaneBody({ windowId, pane, isActive, onActivate, replay, recordPerf }: PaneContentProps) {
  const activity = pane.activities.find((a) => a.id === pane.active_activity);
  if (!activity) return <PanePlaceholder paneId={pane.id} />;

  if (activity.kind === 'extension') {
    return (
      <>
        <ExtensionActivity windowId={windowId} paneId={pane.id} activityId={activity.id} />
        {!isActive && <ClickShield onActivate={onActivate} />}
      </>
    );
  }
  if (activity.kind === 'browser') {
    return (
      <>
        <BrowserActivity windowId={windowId} paneId={pane.id} activityId={activity.id} />
        {!isActive && <ClickShield onActivate={onActivate} />}
      </>
    );
  }
  return (
    <Terminal
      key={activity.id}
      windowId={windowId}
      paneId={pane.id}
      activityId={activity.id}
      isActive={isActive}
      replay={replay}
      recordPerf={recordPerf}
    />
  );
}
