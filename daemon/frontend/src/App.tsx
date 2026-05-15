import { useRef } from 'react';
import { LayoutView } from './layout/LayoutView';
import { useDefaultWindow } from './layout/useDefaultWindow';
import { useWindowLayout } from './layout/useWindowLayout';
import type { ShortcutContext } from './shortcuts/actionDispatch';
import { PrefixIndicator } from './shortcuts/PrefixIndicator';
import { usePrefixMode } from './shortcuts/usePrefixMode';

export function App() {
  const def = useDefaultWindow();
  const wid = def.status === 'ready' ? def.windowId : null;
  const layout = useWindowLayout(wid);

  const view = layout.status === 'gone' ? null : layout.view;
  const activePaneRef = useRef<string | null>(null);
  const activeWindowRef = useRef<string | null>(null);
  const activeActivityRef = useRef<string | null>(null);
  activePaneRef.current = view?.active_pane ?? null;
  activeWindowRef.current = wid;
  const activePaneObj = view?.panes.find((p) => p.id === view.active_pane);
  activeActivityRef.current = activePaneObj?.active_activity ?? null;

  const ctx: ShortcutContext = {
    activeWindow: () => activeWindowRef.current,
    activePane: () => activePaneRef.current,
    activeActivity: () => activeActivityRef.current,
  };

  const { isArmed, prefix } = usePrefixMode(ctx);

  return (
    <>
      <LayoutView windowState={def} layoutState={layout} />
      <PrefixIndicator armed={isArmed} prefix={prefix} />
    </>
  );
}
