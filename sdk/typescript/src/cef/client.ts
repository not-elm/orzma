// Browser-only ambient type — `window.ozmux` is installed by the cef_host
// render process for extension main frames. This SDK module is a thin wrapper
// over that V8 binding: it adds type hints and a stable `createClient()`
// surface that extensions program against.
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

interface NativeOzmux {
  readonly context: OzmuxContext;
  call<Req, Resp>(name: string, payload: Req): Promise<Resp>;
  subscribe<Params, Event>(
    name: string,
    params: Params,
    opts?: SubscribeOptions,
  ): AsyncIterable<Event>;
}

declare const window: { ozmux?: NativeOzmux };

const MISSING =
  'ozmux cef SDK: window.ozmux is missing. The page must be loaded via ozmux-ext:// inside an extension Browser Activity.';

/**
 * Reads the activity context installed by the cef_host render process. Throws
 * if `window.ozmux` is not present (e.g. the page was loaded outside an
 * extension main frame).
 */
export function getOzmuxContext(): OzmuxContext {
  const m = window.ozmux;
  if (!m) throw new Error(MISSING);
  return m.context;
}

/**
 * Returns a thin client over `window.ozmux.call` / `subscribe`. `close()` is
 * a no-op because the underlying V8 binding has no transport-level handle to
 * release — pending operations are GC'd when their iterators / promises drop.
 */
export function createClient(): Client {
  const m = window.ozmux;
  if (!m) throw new Error(MISSING);
  return {
    call: m.call.bind(m),
    subscribe: m.subscribe.bind(m),
    close: () => {},
  };
}
