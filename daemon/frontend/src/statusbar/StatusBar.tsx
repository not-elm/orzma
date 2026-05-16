import type { WindowId } from '../layout/types';
import { ClockSegment } from './ClockSegment';
import { ReconnectPill } from './ReconnectPill';
import { SessionSegment, type SessionSegmentState } from './SessionSegment';
import type { SessionViewState } from './useSessionView';
import { WindowListSegment } from './WindowListSegment';

interface StatusBarProps {
  sessionState: SessionViewState;
  windowReconnecting: boolean;
  onSelectWindow: (wid: WindowId) => void;
}

function deriveSegmentState(s: SessionViewState): SessionSegmentState {
  if (s.status === 'gone') return { status: 'gone', reason: s.reason };
  const view =
    s.status === 'live' || s.status === 'reconnecting' || s.status === 'connecting' ? s.view : null;
  if (!view) return { status: 'loading' };
  return { status: 'ready', name: view.name };
}

/**
 * Composes the four status-bar segments. `sessionState` drives the
 * left and center segments and one of the two reconnect signals;
 * `windowReconnecting` is the second reconnect input. The pill is
 * shown when either reconnect source is active.
 */
export function StatusBar({ sessionState, windowReconnecting, onSelectWindow }: StatusBarProps) {
  const segmentState = deriveSegmentState(sessionState);
  const windows =
    sessionState.status === 'live' || sessionState.status === 'reconnecting'
      ? (sessionState.view?.windows ?? [])
      : [];
  const activeWindowId =
    sessionState.status === 'live' || sessionState.status === 'reconnecting'
      ? (sessionState.view?.active_window ?? null)
      : null;
  const reconnecting = windowReconnecting || sessionState.status === 'reconnecting';

  return (
    <div
      data-testid="status-bar"
      className="flex h-6 shrink-0 items-center gap-3 border-t border-border bg-tmux-status-bar px-2 text-xs"
    >
      <SessionSegment state={segmentState} />
      <WindowListSegment
        windows={windows}
        activeWindowId={activeWindowId}
        onSelect={onSelectWindow}
      />
      <ReconnectPill visible={reconnecting} />
      <ClockSegment />
    </div>
  );
}
