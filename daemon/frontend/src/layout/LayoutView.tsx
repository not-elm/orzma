import { CellNode } from './CellNode';
import { useDefaultWindow } from './useDefaultWindow';
import { useWindowLayout } from './useWindowLayout';

export function LayoutView() {
  const def = useDefaultWindow();
  const wid = def.status === 'ready' ? def.windowId : null;
  const layout = useWindowLayout(wid);

  if (def.status === 'loading') {
    return (
      <div className="flex h-dvh w-dvw items-center justify-center text-muted-foreground">
        Loading…
      </div>
    );
  }
  if (def.status === 'error') {
    return (
      <div className="flex h-dvh w-dvw items-center justify-center p-4 text-destructive">
        Failed to discover window: {def.message}
      </div>
    );
  }
  if (layout.status === 'gone') {
    return (
      <div className="flex h-dvh w-dvw items-center justify-center p-4 text-destructive">
        Window is gone ({layout.reason}).
      </div>
    );
  }
  if (layout.view === null) {
    return (
      <div className="flex h-dvh w-dvw items-center justify-center text-muted-foreground">
        Connecting…
      </div>
    );
  }

  return (
    <div className="relative h-dvh w-dvw bg-background">
      <CellNode node={layout.view.layout} view={layout.view} />
      {layout.status === 'reconnecting' && (
        <div className="absolute right-2 top-2 rounded bg-warning px-2 py-1 text-xs text-warning-foreground">
          Reconnecting…
        </div>
      )}
    </div>
  );
}
