import { useRef } from 'react';
import { LayoutView } from './layout/LayoutView';
import type { DefaultWindowState } from './layout/types';
import { useWindowLayout } from './layout/useWindowLayout';
import type { ShortcutContext } from './shortcuts/actionDispatch';
import { PrefixIndicator } from './shortcuts/PrefixIndicator';
import { usePrefixMode } from './shortcuts/usePrefixMode';
import { StatusBar } from './statusbar/StatusBar';
import type { SessionView } from './statusbar/types';
import { useDefaultSession } from './statusbar/useDefaultSession';
import { liveOrReconnectingView, useSessionView } from './statusbar/useSessionView';
import { windowSelect } from './statusbar/windowSelect';

export function App() {
  const sessionDefault = useDefaultSession();
  const sid = sessionDefault.status === 'ready' ? sessionDefault.sessionId : null;
  const sessionView = useSessionView(sid);

  const wid = liveOrReconnectingView(sessionView)?.active_window ?? null;

  const layout = useWindowLayout(wid);

  const def: DefaultWindowState =
    wid !== null
      ? { status: 'ready', windowId: wid }
      : sessionDefault.status === 'error'
        ? { status: 'error', message: sessionDefault.message }
        : { status: 'loading' };

  const view = layout.status === 'gone' ? null : layout.view;
  const activePaneRef = useRef<string | null>(null);
  const activeWindowRef = useRef<string | null>(null);
  const activeActivityRef = useRef<string | null>(null);
  const activeSessionRef = useRef<SessionView | null>(null);
  activePaneRef.current = view?.active_pane ?? null;
  activeWindowRef.current = wid;
  const activePaneObj = view?.panes.find((p) => p.id === view.active_pane);
  activeActivityRef.current = activePaneObj?.active_activity ?? null;
  activeSessionRef.current = liveOrReconnectingView(sessionView);

  const ctx: ShortcutContext = {
    activeWindow: () => activeWindowRef.current,
    activePane: () => activePaneRef.current,
    activeActivity: () => activeActivityRef.current,
    activeSession: () => activeSessionRef.current,
  };

  const { isArmed, prefix } = usePrefixMode(ctx);

  return (
    <div className="flex h-dvh w-dvw flex-col bg-background">
      <div className="relative min-h-0 flex-1">
        <LayoutView windowState={def} layoutState={layout} />
      </div>
      <StatusBar
        sessionState={sessionView}
        windowReconnecting={layout.status === 'reconnecting'}
        onSelectWindow={windowSelect}
      />
      <PrefixIndicator armed={isArmed} prefix={prefix} />
    </div>
  );
}
