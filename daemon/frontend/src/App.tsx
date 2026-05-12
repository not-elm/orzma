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
  activePaneRef.current = view?.active_pane ?? null;

  const bindings: PrefixBindings = new Map([
    [
      'x',
      () => {
        const pid = activePaneRef.current;
        if (pid) closePane(pid);
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
