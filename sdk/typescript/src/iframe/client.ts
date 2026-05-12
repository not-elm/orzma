import type {
  HandlerCallFrame,
  HandlerErrorFrame,
  HandlerResultFrame,
  SubCancelFrame,
  SubCompleteFrame,
  SubDataFrame,
  SubErrorFrame,
  SubOpenFrame,
} from "../server/protocol.ts";

// Minimal ambient types — this SDK module targets the browser but the SDK
// package itself does not depend on `lib: ["DOM"]` (server code shares the
// tsconfig). Declare the small surface we use.
interface MinimalWebSocket {
  readonly readyState: 0 | 1 | 2 | 3;
  send(data: string): void;
  close(): void;
  onopen: (() => void) | null;
  onclose: (() => void) | null;
  onerror: (() => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
}
interface MinimalWebSocketCtor {
  new (url: string): MinimalWebSocket;
}
interface MinimalLocation {
  protocol: string;
  host: string;
  pathname: string;
}
interface MinimalAbortSignal {
  readonly aborted: boolean;
  addEventListener(
    type: "abort",
    listener: () => void,
    options?: { once?: boolean },
  ): void;
  removeEventListener(type: "abort", listener: () => void): void;
}

// The daemon injects this script tag into the iframe HTML before serving:
//   <script>window.__OZMUX__ = {sessionId, windowId, paneId, activityId};</script>
// so the SDK can know its position in the hierarchy without parsing the URL.
declare const WebSocket: MinimalWebSocketCtor;
declare const window: {
  location: MinimalLocation;
  __OZMUX__?: OzmuxContext;
};

export interface OzmuxContext {
  sessionId: string | null;
  windowId: string;
  paneId: string;
  activityId: string;
}

/**
 * Read the daemon-injected `(sessionId, windowId, paneId, activityId)` tuple
 * from `window.__OZMUX__`. Throws if the global is absent — that means the
 * iframe was loaded outside the ozmux hierarchical route and the SDK has no
 * way to discover its identity.
 */
export function getOzmuxContext(): OzmuxContext {
  const ctx = window.__OZMUX__;
  if (!ctx) {
    throw new Error(
      "ozmux iframe SDK: window.__OZMUX__ not found. The iframe must be served via /windows/{wid}/panes/{pid}/activities/{aid}/iframe/...",
    );
  }
  return ctx;
}

const WS_OPEN = 1;
const WS_CLOSED = 3;

export interface CreateClientOptions {
  /**
   * Override the handlers WS URL. Useful for tests; production callers should
   * omit this and let the SDK build the hierarchical URL from
   * `window.__OZMUX__`.
   */
  url?: string;
}

export interface SubscribeOptions {
  signal?: MinimalAbortSignal;
}

export class ConnectionClosedError extends Error {
  constructor() {
    super("ozmux iframe SDK: WebSocket connection closed");
    this.name = "ConnectionClosedError";
  }
}

interface PendingCall {
  resolve: (v: unknown) => void;
  reject: (e: unknown) => void;
}

interface SubState {
  queue: unknown[];
  done: boolean;
  error: Error | null;
  waker: { resolve: () => void } | null;
  detach: () => void;
  cancel: () => void;
}

export interface Client {
  call<Req, Resp>(name: string, payload: Req): Promise<Resp>;
  subscribe<Params, Event>(
    name: string,
    params: Params,
    opts?: SubscribeOptions,
  ): AsyncIterable<Event>;
  close(): void;
}

function handlersWsUrl(ctx: OzmuxContext): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${window.location.host}/windows/${ctx.windowId}/panes/${ctx.paneId}/activities/${ctx.activityId}/handlers/ws`;
}

function toRpcError(frame: HandlerErrorFrame | SubErrorFrame): Error {
  const err = new Error(frame.message) as Error & { code?: string };
  err.code = frame.code;
  return err;
}

export function createClient(opts: CreateClientOptions = {}): Client {
  // Production callers omit `opts.url` and let the SDK build a hierarchical
  // URL from the daemon-injected `window.__OZMUX__`. Tests override `url` to
  // point at a mock WebSocket.
  const url = opts.url ?? handlersWsUrl(getOzmuxContext());
  const ws = new WebSocket(url);

  const pendingCalls = new Map<string, PendingCall>();
  const subs = new Map<string, SubState>();
  const outbox: string[] = [];
  let seq = 0;
  const nextId = () => `c${seq++}`;
  const isOpen = () => ws.readyState === WS_OPEN;
  const isClosed = () => ws.readyState === WS_CLOSED;

  const send = (frame: HandlerCallFrame | SubOpenFrame | SubCancelFrame) => {
    const line = JSON.stringify(frame);
    if (isOpen()) ws.send(line);
    else outbox.push(line);
  };

  const failAll = () => {
    const err = new ConnectionClosedError();
    for (const p of pendingCalls.values()) p.reject(err);
    pendingCalls.clear();
    for (const s of subs.values()) {
      s.detach();
      s.error = err;
      s.done = true;
      s.waker?.resolve();
    }
    subs.clear();
  };

  const finishSub = (id: string, error: Error | null) => {
    const s = subs.get(id);
    if (!s) return;
    s.detach();
    s.error = error;
    s.done = true;
    s.waker?.resolve();
    subs.delete(id);
  };

  ws.onopen = () => {
    for (const line of outbox.splice(0)) ws.send(line);
  };
  ws.onclose = () => failAll();
  ws.onerror = () => {
    // Errors precede `close`; cleanup happens there.
  };
  ws.onmessage = (ev) => {
    if (typeof ev.data !== "string") return;
    let f:
      | HandlerResultFrame
      | HandlerErrorFrame
      | SubDataFrame
      | SubCompleteFrame
      | SubErrorFrame;
    try {
      f = JSON.parse(ev.data);
    } catch {
      return;
    }
    switch (f.kind) {
      case "result": {
        const p = pendingCalls.get(f.id);
        if (!p) return;
        pendingCalls.delete(f.id);
        p.resolve(f.payload);
        return;
      }
      case "error": {
        const p = pendingCalls.get(f.id);
        if (!p) return;
        pendingCalls.delete(f.id);
        p.reject(toRpcError(f));
        return;
      }
      case "sub.data": {
        const s = subs.get(f.id);
        if (!s || s.done) return;
        s.queue.push(f.payload);
        s.waker?.resolve();
        return;
      }
      case "sub.complete":
        finishSub(f.id, null);
        return;
      case "sub.error":
        finishSub(f.id, toRpcError(f));
        return;
    }
  };

  return {
    call<Req, Resp>(name: string, payload: Req): Promise<Resp> {
      if (isClosed()) return Promise.reject(new ConnectionClosedError());
      const id = nextId();
      return new Promise<Resp>((resolve, reject) => {
        pendingCalls.set(id, {
          resolve: (v) => resolve(v as Resp),
          reject,
        });
        send({ kind: "call", id, name, payload });
      });
    },

    subscribe<Params, Event>(
      name: string,
      params: Params,
      opts: SubscribeOptions = {},
    ): AsyncIterable<Event> {
      const id = nextId();
      const signal = opts.signal;
      let onAbort: (() => void) | null = null;
      const state: SubState = {
        queue: [],
        done: false,
        error: null,
        waker: null,
        detach: () => {
          if (onAbort && signal) {
            signal.removeEventListener("abort", onAbort);
            onAbort = null;
          }
        },
        cancel: () => {
          if (state.done) return;
          state.done = true;
          state.detach();
          subs.delete(id);
          send({ kind: "sub.cancel", id });
        },
      };

      if (signal?.aborted) {
        state.done = true;
      } else {
        subs.set(id, state);
        send({ kind: "sub.open", id, name, params });
        if (signal) {
          onAbort = () => {
            state.cancel();
            state.waker?.resolve();
          };
          signal.addEventListener("abort", onAbort, { once: true });
        }
      }

      return {
        [Symbol.asyncIterator](): AsyncIterator<Event> {
          return {
            async next(): Promise<IteratorResult<Event>> {
              while (true) {
                if (state.queue.length > 0) {
                  const v = state.queue.shift() as Event;
                  return { value: v, done: false };
                }
                if (state.error) throw state.error;
                if (state.done) return { value: undefined, done: true };
                await new Promise<void>((resolve) => {
                  state.waker = { resolve };
                });
                state.waker = null;
              }
            },
            async return(): Promise<IteratorResult<Event>> {
              state.cancel();
              return { value: undefined, done: true };
            },
          };
        },
      };
    },

    close() {
      if (isClosed()) return;
      ws.close();
      failAll();
    },
  };
}
