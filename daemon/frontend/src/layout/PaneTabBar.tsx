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
        'flex shrink-0 overflow-hidden bg-tmux-status-bar',
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
              'min-w-0 flex-1 truncate border-r border-tmux-pane-border px-3 py-1',
              'text-left font-mono text-xs',
              selected
                ? 'border-t border-t-tmux-pane-active bg-background text-tmux-pane-active'
                : 'text-muted-foreground',
            )}
          >
            {activity.title}
          </button>
        );
      })}
    </div>
  );
}
