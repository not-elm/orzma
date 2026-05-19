import { useRef } from 'react';
import { ChooseTreeOverlay } from './choose-tree/ChooseTreeOverlay';
import { useChooseTree } from './choose-tree/useChooseTree';
import { LayoutView } from './layout/LayoutView';
import type { DefaultWindowState } from './layout/types';
import { useWindowLayout } from './layout/useWindowLayout';
import type { ShortcutContext } from './shortcuts/actionDispatch';
import { PrefixIndicator } from './shortcuts/PrefixIndicator';
import { usePrefixMode } from './shortcuts/usePrefixMode';
import { RenameWindowPrompt } from './statusbar/RenameWindowPrompt';
import { StatusBar } from './statusbar/StatusBar';
import type { SessionView } from './statusbar/types';
import { useAttachedSession } from './statusbar/useAttachedSession';
import { useRenameWindowPrompt } from './statusbar/useRenameWindowPrompt';
import { liveOrReconnectingView, useSessionView } from './statusbar/useSessionView';
import { windowSelect } from './statusbar/windowSelect';

/** Top-level app props: dev-only replay/record-perf flags threaded down to the WS layer. */
export interface AppProps {
  replay?: string;
  recordPerf?: boolean;
}

export function App({ replay, recordPerf }: AppProps = {}) {
  const attached = useAttachedSession();
  const sid = attached.status === 'ready' ? attached.sessionId : null;
  const sessionView = useSessionView(sid);

  const liveSession = liveOrReconnectingView(sessionView);
  const wid = liveSession?.active_window ?? null;

  const layout = useWindowLayout(wid);

  const def: DefaultWindowState =
    wid !== null
      ? { status: 'ready', windowId: wid }
      : attached.status === 'error'
        ? { status: 'error', message: attached.message }
        : { status: 'loading' };

  const view = layout.status === 'gone' ? null : layout.view;
  const activePaneRef = useRef<string | null>(null);
  const activeWindowRef = useRef<string | null>(null);
  const activeActivityRef = useRef<string | null>(null);
  const activeSessionRef = useRef<SessionView | null>(null);
  const activeWindowNameRef = useRef<string | null>(null);
  activePaneRef.current = view?.active_pane ?? null;
  activeWindowRef.current = wid;
  const activePaneObj = view?.panes.find((p) => p.id === view.active_pane);
  activeActivityRef.current = activePaneObj?.active_activity ?? null;
  activeSessionRef.current = liveSession;
  activeWindowNameRef.current = liveSession?.windows.find((w) => w.id === wid)?.name ?? null;

  const { promptState, openPrompt, closePrompt } = useRenameWindowPrompt();
  const openPromptRef = useRef(openPrompt);
  openPromptRef.current = openPrompt;

  const chooseTree = useChooseTree();
  const openChooseTreeRef = useRef(chooseTree.open);
  openChooseTreeRef.current = chooseTree.open;

  // NOTE: usePrefixMode caches the shortcut handlers at mount with `useEffect([])`,
  // so the `ctx.openChooseTree` closure runs forever against the values it captured
  // on first render. attached.status is 'loading' at first render — without this
  // ref the guard below permanently early-returns and the picker never opens in
  // production (Vite HMR masks the bug in dev).
  const attachedRef = useRef(attached);
  attachedRef.current = attached;

  const ctx: ShortcutContext = {
    activeWindow: () => activeWindowRef.current,
    activePane: () => activePaneRef.current,
    activeActivity: () => activeActivityRef.current,
    activeSession: () => activeSessionRef.current,
    openRenameWindow: () => {
      const w = activeWindowRef.current;
      const name = activeWindowNameRef.current;
      if (w === null || name === null) return;
      openPromptRef.current(w, name);
    },
    openChooseTree: () => {
      if (attachedRef.current.status !== 'ready') return;
      openChooseTreeRef.current();
    },
  };

  const { isArmed, prefix } = usePrefixMode(ctx);

  return (
    <div className="flex h-dvh w-dvw flex-col bg-background">
      <div className="relative min-h-0 flex-1">
        <LayoutView
          windowState={def}
          layoutState={layout}
          replay={replay}
          recordPerf={recordPerf}
        />
      </div>
      <RenameWindowPrompt promptState={promptState} closePrompt={closePrompt} />
      {chooseTree.state.open && attached.status === 'ready' && (
        <ChooseTreeOverlay
          onClose={chooseTree.close}
          attachedSessionId={attached.sessionId}
          setAttachedSession={attached.setSession}
        />
      )}
      <StatusBar
        sessionState={sessionView}
        windowReconnecting={layout.status === 'reconnecting'}
        onSelectWindow={windowSelect}
      />
      <PrefixIndicator armed={isArmed} prefix={prefix} />
    </div>
  );
}
