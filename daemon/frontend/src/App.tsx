import { useRef } from 'react';
import { closePane } from './layout/closePane';
import { LayoutView } from './layout/LayoutView';
import { useDefaultWindow } from './layout/useDefaultWindow';
import { useWindowLayout } from './layout/useWindowLayout';
import { PrefixIndicator } from './shortcuts/PrefixIndicator';
import { type PrefixBindings, usePrefixMode } from './shortcuts/usePrefixMode';

export function App() {
  const def = useDefaultWindow();
  const wid = def.status === 'ready' ? def.windowId : null;
  const layout = useWindowLayout(wid);

  const view = layout.status === 'gone' ? null : layout.view;
  const activePaneRef = useRef<string | null>(null);
  const activeWindowRef = useRef<string | null>(null);
  activePaneRef.current = view?.active_pane ?? null;
  activeWindowRef.current = wid;

  const bindings: PrefixBindings = new Map([
    [
      'x',
      () => {
        const pid = activePaneRef.current;
        const w = activeWindowRef.current;
        if (pid && w) closePane(w, pid);
      },
    ],
  ]);

  const { isArmed } = usePrefixMode(bindings);

  return (
    <>
      <LayoutView windowState={def} layoutState={layout} />
      <PrefixIndicator armed={isArmed} />
    </>
  );
}
