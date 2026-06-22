/** ECMAScript-erasable only (Node type-stripping): no enum, namespace, or param-properties. */

/** The page-side bridge the host injects as `window.ozma`. */
export interface OzmaApi {
  /**
   * Invokes a host-routed method and resolves with its reply.
   *
   * Supply `<R>` for the reply type — it sits in a return position and cannot be
   * inferred; `<P>` (params) is inferred from the argument. A top-level
   * `Uint8Array` in `params` and in the resolved value round-trips (the bridge
   * base64-tags it). NOTE: bytes nested inside an object or array do NOT
   * round-trip and are silently lost.
   */
  call<R = unknown, P = unknown>(method: string, params?: P): Promise<R>;
  /**
   * Subscribes `handler` to a named host event.
   *
   * Annotate the handler parameter (`(payload: T) => …`) to type `payload`; `P`
   * is inferred from that annotation and defaults to `unknown`.
   */
  on<P = unknown>(event: string, handler: (payload: P) => void): void;
  /**
   * Removes a previously-registered event handler by reference equality.
   *
   * `P` is generic so a typed handler stays assignable here under
   * `strictFunctionTypes`; it should match the type used at `on`.
   */
  off<P = unknown>(event: string, handler: (payload: P) => void): void;
  /**
   * Sends a one-way event to the host app (fire-and-forget; no reply).
   *
   * `P` (payload) is inferred from the argument and defaults to `unknown`.
   */
  emit<P = unknown>(event: string, payload?: P): void;
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
  call<R = unknown, P = unknown>(method: string, params?: P): Promise<R> {
    return resolve().call<R>(method, params);
  },
  on<P = unknown>(event: string, handler: (payload: P) => void): void {
    resolve().on(event, handler);
  },
  off<P = unknown>(event: string, handler: (payload: P) => void): void {
    resolve().off(event, handler);
  },
  emit<P = unknown>(event: string, payload?: P): void {
    resolve().emit(event, payload);
  },
};

/** Reports whether the host bridge (`window.ozma`) is present. */
export function isOzmaAvailable(): boolean {
  return currentBridge() !== undefined;
}
