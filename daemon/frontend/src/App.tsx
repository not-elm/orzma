import { LayoutView } from './layout/LayoutView';
import { useDefaultWindow } from './layout/useDefaultWindow';
import { useWindowLayout } from './layout/useWindowLayout';

export function App() {
  const def = useDefaultWindow();
  const wid = def.status === 'ready' ? def.windowId : null;
  const layout = useWindowLayout(wid);
  return <LayoutView windowState={def} layoutState={layout} />;
}
