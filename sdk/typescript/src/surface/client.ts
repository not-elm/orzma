// Type-declaration module for the host-injected `window.ozmux` binding. The
// runtime that backs these types lives in `ozmux-bridge.ts` / `ozmux.js` and is
// installed on `window.ozmux` by the cef_host render process for extension main
// frames; this file only declares the surface extensions program against.
//
// The SDK package's tsconfig deliberately excludes the DOM lib (server code
// shares the same tsconfig). Declare the small browser surface we use.

interface MinimalAbortSignal {
  readonly aborted: boolean;
  addEventListener(type: 'abort', listener: () => void, options?: { once?: boolean }): void;
}

/** Per-extension activity context populated by the daemon at browser create. */
export interface OzmuxContext {
  sessionId: string | null;
  windowId: string;
  paneId: string;
  activityId: string;
  /** Role of the surrounding browser — always `"extension"` for this binding. */
  role: 'extension';
  /** Owning extension name (empty/undefined for non-extension browsers). */
  extensionName?: string;
}

/** Subscribe call options. */
export interface SubscribeOptions {
  signal?: MinimalAbortSignal;
}

/** Client surface exposed to extension code. */
export interface Client {
  call<Req, Resp>(name: string, payload: Req): Promise<Resp>;
  subscribe<Params, Event>(
    name: string,
    params: Params,
    opts?: SubscribeOptions,
  ): AsyncIterable<Event>;
  close(): void;
}
