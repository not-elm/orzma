import { useEffect, useRef } from 'react';
import type { WindowId } from '../layout/types';
import type { SessionWindowEntry } from './types';

interface WindowListSegmentProps {
  windows: SessionWindowEntry[];
  activeWindowId: WindowId | null;
  onSelect: (wid: WindowId) => void;
}

/**
 * Horizontally-scrollable list of windows. Each entry is a button that
 * promotes its window to active on click. The active chip is
 * `aria-current="page"` and is scrolled into view on every change.
 */
export function WindowListSegment({ windows, activeWindowId, onSelect }: WindowListSegmentProps) {
  const activeRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }, [activeWindowId]);

  return (
    <nav
      aria-label="Windows"
      className="flex flex-1 items-center gap-4 overflow-x-auto"
    >
      {windows.map((w) => {
        const isActive = w.id === activeWindowId;
        return (
          <button
            type="button"
            key={w.id}
            ref={isActive ? activeRef : undefined}
            aria-current={isActive ? 'page' : undefined}
            aria-label={`Switch to window ${w.index}: ${w.name}`}
            onClick={() => onSelect(w.id)}
            className={
              isActive
                ? 'whitespace-nowrap font-semibold text-tmux-status-bar-foreground'
                : 'whitespace-nowrap text-muted-foreground'
            }
          >
            {`${w.index}:${w.name}${isActive ? '*' : ''}`}
          </button>
        );
      })}
    </nav>
  );
}
