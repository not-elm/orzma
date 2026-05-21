//! Browser Activity — Toolbar + shared {@link CefCanvas} surface.
//!
//! All canvas / worker / input / WebSocket plumbing lives in `CefCanvas`,
//! which is also reused by Extension activities. This wrapper is responsible
//! only for the Browser-specific chrome: a URL/back/forward Toolbar above the
//! viewport and the right-click ContextMenu overlay.

import { CefCanvas } from '../cef/CefCanvas';
import { getBrowserConfig } from '../config/browser';
import { ContextMenu } from './ContextMenu';
import { Toolbar } from './Toolbar';

interface Props {
  windowId: string;
  paneId: string;
  activityId: string;
}

/** Browser Activity — wraps the shared {@link CefCanvas} with a Toolbar and
 *  right-click ContextMenu. The Extension Activity uses the same canvas
 *  without these chrome elements. */
export function BrowserActivity({ windowId, paneId, activityId }: Props) {
  return (
    <CefCanvas
      windowId={windowId}
      paneId={paneId}
      activityId={activityId}
      path="browser/ws"
      renderHeader={({ nav, send }) => (
        <Toolbar
          url={nav.url}
          canBack={nav.can_back}
          canForward={nav.can_forward}
          searchTemplate={getBrowserConfig().searchTemplate}
          onBack={() => send({ kind: 'navigate_history', delta: -1 })}
          onForward={() => send({ kind: 'navigate_history', delta: 1 })}
          onReload={() => send({ kind: 'navigate', url: nav.url })}
          onGo={(url) => send({ kind: 'navigate', url })}
        />
      )}
      renderContextMenu={({ x, y, nav, send, close }) => (
        <ContextMenu
          x={x}
          y={y}
          onClose={close}
          onBack={() => send({ kind: 'navigate_history', delta: -1 })}
          onForward={() => send({ kind: 'navigate_history', delta: 1 })}
          onReload={() => send({ kind: 'navigate', url: nav.url })}
          // TODO: Plan 3 wires the full clipboard round-trip for the CEF path.
          onCopy={() => send({ kind: 'copy_request' })}
          onPaste={() => {
            navigator.clipboard.readText().then(
              (t) => send({ kind: 'paste', text: t }),
              () => {
                // NOTE: clipboard read may be denied (permissions, focus) — ignore.
              },
            );
          }}
        />
      )}
    />
  );
}
