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
  addEventListener(type: "abort", listener: () => void): void;
}

declare const WebSocket: MinimalWebSocketCtor;
declare const window: { location: MinimalLocation };

export interface CreateClientOptions {
  activityId?: string;
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

function inferActivityId(): string {
  const m = window.location.pathname.match(/^\/activities\/([^/]+)\/iframe\//);
  if (!m) {
    throw new Error(
      "ozmux iframe SDK: cannot infer activityId from pathname; pass activityId explicitly",
    );
  }
  return m[1]!;
}

function inferUrl(aid: string): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${window.location.host}/activities/${aid}/handlers/ws`;
}

export function createClient(opts: CreateClientOptions = {}): Client {
  const aid = opts.activityId ?? inferActivityId();
  const url = opts.url ?? inferUrl(aid);
  const ws = new WebSocket(url);

  const pendingCalls = new Map<string, PendingCall>();
  const subs = new Map<string, SubState>();
  const outbox: string[] = [];
  let open = false;
  let closed = false;
  let seq = 0;
  const nextId = () => `c${seq++}`;

  const flush = () => {
    if (!open) return;
    for (const line of outbox.splice(0)) ws.send(line);
  };

  const send = (frame: HandlerCallFrame | SubOpenFrame | SubCancelFrame) => {
    const line = JSON.stringify(frame);
    if (open) ws.send(line);
    else outbox.push(line);
  };

  const failAll = () => {
    const err = new ConnectionClosedError();
    for (const p of pendingCalls.values()) p.reject(err);
    pendingCalls.clear();
    for (const s of subs.values()) {
      s.error = err;
      s.done = true;
      s.waker?.resolve();
    }
    subs.clear();
  };

  ws.onopen = () => {
    open = true;
    flush();
  };
  ws.onclose = () => {
    if (closed) return;
    closed = true;
    open = false;
    failAll();
  };
  ws.onerror = () => {
    if (closed) return;
  };
  ws.onmessage = (ev) => {
    let f:
      | HandlerResultFrame
      | HandlerErrorFrame
      | SubDataFrame
      | SubCompleteFrame
      | SubErrorFrame;
    try {
      f = JSON.parse(typeof ev.data === "string" ? ev.data : "");
    } catch {
      return;
    }
    if (f.kind === "result") {
      const p = pendingCalls.get(f.id);
      if (p) {
        pendingCalls.delete(f.id);
        p.resolve(f.payload);
      }
      return;
    }
    if (f.kind === "error") {
      const p = pendingCalls.get(f.id);
      if (p) {
        pendingCalls.delete(f.id);
        const err = new Error(f.message);
        (err as Error & { code?: string }).code = f.code;
        p.reject(err);
      }
      return;
    }
    if (f.kind === "sub.data") {
      const s = subs.get(f.id);
      if (!s || s.done) return;
      s.queue.push(f.payload);
      s.waker?.resolve();
      return;
    }
    if (f.kind === "sub.complete") {
      const s = subs.get(f.id);
      if (!s) return;
      s.done = true;
      s.waker?.resolve();
      subs.delete(f.id);
      return;
    }
    if (f.kind === "sub.error") {
      const s = subs.get(f.id);
      if (!s) return;
      const err = new Error(f.message);
      (err as Error & { code?: string }).code = f.code;
      s.error = err;
      s.done = true;
      s.waker?.resolve();
      subs.delete(f.id);
      return;
    }
  };

  return {
    call<Req, Resp>(name: string, payload: Req): Promise<Resp> {
      if (closed) return Promise.reject(new ConnectionClosedError());
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
      const state: SubState = {
        queue: [],
        done: false,
        error: null,
        waker: null,
        cancel: () => {
          if (state.done) return;
          state.done = true;
          subs.delete(id);
          send({ kind: "sub.cancel", id });
        },
      };

      const startAborted = signal?.aborted === true;
      if (startAborted) {
        state.done = true;
      } else {
        subs.set(id, state);
        send({ kind: "sub.open", id, name, params });
        signal?.addEventListener("abort", () => {
          state.cancel();
          state.waker?.resolve();
        });
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
      if (closed) return;
      closed = true;
      open = false;
      ws.close();
      failAll();
    },
  };
}
