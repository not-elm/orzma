/** ECMAScript-erasable only (Node type-stripping): no enum, namespace, or param-properties. */

/** The page-side bridge the host injects as `window.ozma`. */
export interface OzmaApi {
  /**
   * Invokes a host-routed method and resolves with its reply.
   *
   * A top-level `Uint8Array` in `params` and in the resolved value round-trips
   * (the bridge base64-tags it). NOTE: bytes nested inside an object or array do
   * NOT round-trip and are silently lost.
   */
  call<R = unknown>(method: string, params?: unknown): Promise<R>;
  /** Subscribes `handler` to a named host event. */
  on(event: string, handler: (payload: unknown) => void): void;
  /** Removes a previously-registered event handler by reference equality. */
  off(event: string, handler: (payload: unknown) => void): void;
}

// NOTE: augments `window.ozma` for browser consumers whose tsconfig includes the "dom" lib;
// the runtime path reads `globalThis` (currentBridge) and does not depend on this declaration.
declare global {
  interface Window {
    ozma?: OzmaApi;
  }
}

function currentBridge(): OzmaApi | undefined {
  return (globalThis as typeof globalThis & { ozma?: OzmaApi }).ozma;
}

function resolve(): OzmaApi {
  const api = currentBridge();
  if (api === undefined) {
    throw new Error('window.ozma is unavailable: run this inside an ozma-bridged webview');
  }
  return api;
}

/** Typed accessor for the host-injected `window.ozma` bridge; throws if absent. */
export const ozma: OzmaApi = {
  call<R = unknown>(method: string, params?: unknown): Promise<R> {
    return resolve().call<R>(method, params);
  },
  on(event: string, handler: (payload: unknown) => void): void {
    resolve().on(event, handler);
  },
  off(event: string, handler: (payload: unknown) => void): void {
    resolve().off(event, handler);
  },
};

/** Reports whether the host bridge (`window.ozma`) is present. */
export function isOzmaAvailable(): boolean {
  return currentBridge() !== undefined;
}
