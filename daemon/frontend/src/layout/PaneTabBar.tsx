import { clsx } from 'clsx';
import type { PointerEvent } from 'react';
import { activateActivity } from './activateActivity';
import type { ActivityId, PaneView } from './types';

interface PaneTabBarProps {
  windowId: string;
  pane: PaneView;
  isActive: boolean;
  onActivate: () => void;
}

export function PaneTabBar({ windowId, pane, isActive, onActivate }: PaneTabBarProps) {
  const selectTab = (event: PointerEvent<HTMLButtonElement>, activityId: ActivityId) => {
    // Do not let the click bubble to the pane container's activate handler.
    event.stopPropagation();
    // Pane focus first, then the activity switch — a single defined order.
    if (!isActive) onActivate();
    if (activityId === pane.active_activity) return;
    void activateActivity(windowId, pane.id, activityId);
  };

  return (
    <div
      role="tablist"
      className={clsx(
        'flex shrink-0 gap-0.5 overflow-hidden bg-tmux-status-bar px-1 pt-1',
        !isActive && 'opacity-60',
      )}
    >
      {pane.activities.map((activity) => {
        const selected = activity.id === pane.active_activity;
        return (
          <button
            key={activity.id}
            type="button"
            role="tab"
            aria-selected={selected}
            title={activity.title}
            onPointerDown={(event) => selectTab(event, activity.id)}
            className={clsx(
              'w-35 shrink-0 truncate border-x border-t px-3 py-1 text-left font-mono text-xs',
              selected
                ? 'border-tmux-pane-border bg-background text-foreground'
                : 'border-transparent bg-tmux-tab-inactive-bg text-tmux-tab-inactive-foreground',
            )}
          >
            {activity.title}
          </button>
        );
      })}
    </div>
  );
}
