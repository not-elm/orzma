import { useEffect, useRef } from 'react';
import { Terminal } from '../terminal/Terminal';
import { ClickShield } from './ClickShield';
import { PanePlaceholder } from './PanePlaceholder';
import type { PaneView } from './types';
import { useIframeKeydownBridge } from './useIframeKeydownBridge';

interface PaneContentProps {
  pane: PaneView;
  isActive: boolean;
  onActivate: () => void;
}

export function PaneContent({ pane, isActive, onActivate }: PaneContentProps) {
  const activity = pane.activities.find((a) => a.id === pane.active_activity);
  if (!activity) return <PanePlaceholder paneId={pane.id} />;

  if (activity.kind === 'extension') {
    if (!activity.iframe_url) return <PanePlaceholder paneId={pane.id} />;
    return (
      <IframePane
        url={activity.iframe_url}
        title={`extension-${activity.id}`}
        isActive={isActive}
        onActivate={onActivate}
      />
    );
  }
  return <Terminal key={activity.id} activityId={activity.id} isActive={isActive} />;
}

interface IframePaneProps {
  url: string;
  title: string;
  isActive: boolean;
  onActivate: () => void;
}

function IframePane({ url, title, isActive, onActivate }: IframePaneProps) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const prevActiveRef = useRef(isActive);
  useEffect(() => {
    if (isActive && !prevActiveRef.current) {
      iframeRef.current?.contentWindow?.focus();
    }
    prevActiveRef.current = isActive;
  }, [isActive]);

  useIframeKeydownBridge(iframeRef);

  return (
    <>
      <iframe ref={iframeRef} src={url} title={title} className="h-full w-full border-0" />
      {!isActive && <ClickShield onActivate={onActivate} />}
    </>
  );
}
