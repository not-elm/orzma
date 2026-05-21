//! Extension Activity — toolbar-less {@link CefCanvas}.
//!
//! Extension UIs run in their own out-of-process CEF browser (one per
//! activity), dispatched by the daemon over `/extension/cef/ws`. The wire
//! protocol is identical to the Browser activity's `/browser/ws`, so the
//! same `CefCanvas` renders both; the Extension wrapper simply omits the
//! Browser URL/back/forward Toolbar.

import { CefCanvas } from '../cef/CefCanvas';

interface Props {
  windowId: string;
  paneId: string;
  activityId: string;
}

/** Extension Activity — renders the shared {@link CefCanvas} against the
 *  `/extension/cef/ws` endpoint, with no Toolbar chrome. */
export function ExtensionActivity({ windowId, paneId, activityId }: Props) {
  return (
    <CefCanvas
      windowId={windowId}
      paneId={paneId}
      activityId={activityId}
      path="extension/cef/ws"
    />
  );
}
