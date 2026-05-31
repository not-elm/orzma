/** The bevy_cef JS primitives the bridge rides on (provided in-page). */
export interface CefApi {
  // NOTE: bevy_cef's cef.emit serializes only its FIRST argument into one global
  // Receive<E> on the Rust side — there is no channel-name arg (a second arg is
  // silently dropped). cef.listen IS id-routed (Rust→JS keys by event id).
  emit(payload: unknown): void;
  listen(id: string, cb: (raw: unknown) => void): void;
}

interface Pending {
  resolve(v: unknown): void;
  reject(e: Error): void;
}

interface SubState {
  queue: unknown[];
  waiter?: { resolve: (r: IteratorResult<unknown>) => void; reject: (e: Error) => void };
  done: boolean;
  error?: Error;
}

/** The `window.ozmux` surface consumed by `cef/client.ts`. */
export interface OzmuxApi {
  call(name: string, payload: unknown): Promise<unknown>;
  subscribe(name: string, params: unknown, opts?: { signal?: AbortSignal }): AsyncIterable<unknown>;
}

/**
 * Builds the `window.ozmux` bridge over `cef.emit`/`cef.listen`, correlating
 * server frames by id. `HostEmitEvent` delivers payloads as JSON strings, so
 * the inbound listener parses defensively.
 */
export function installOzmux(cef: CefApi): OzmuxApi {
  let nextId = 0;
  const calls = new Map<string, Pending>();
  const subs = new Map<string, SubState>();

  cef.listen('ozmux', (raw) => {
    // NOTE: HostEmitEvent delivers the Rust→JS payload as a JSON string, not an object.
    const frame: any = typeof raw === 'string' ? JSON.parse(raw) : raw;
    const id = frame.id as string;
    switch (frame.kind) {
      case 'result':
        calls.get(id)?.resolve(frame.payload);
        calls.delete(id);
        break;
      case 'error':
        calls.get(id)?.reject(new Error(`${frame.code}: ${frame.message}`));
        calls.delete(id);
        break;
      case 'sub.data':
        pushSub(subs.get(id), frame.payload);
        break;
      case 'sub.complete':
        endSub(id);
        break;
      case 'sub.error':
        endSub(id, new Error(`${frame.code}: ${frame.message}`));
        break;
    }
  });

  return {
    call(name, payload) {
      const id = `c${nextId++}`;
      return new Promise((resolve, reject) => {
        calls.set(id, { resolve, reject });
        cef.emit({ kind: 'call', id, name, payload });
      });
    },
    subscribe(name, params, opts) {
      const id = `s${nextId++}`;
      const state: SubState = { queue: [], done: false };
      subs.set(id, state);
      cef.emit({ kind: 'sub.open', id, name, params });
      opts?.signal?.addEventListener('abort', () => {
        cef.emit({ kind: 'sub.cancel', id });
        endSub(id);
      });
      return {
        [Symbol.asyncIterator]() {
          return {
            // NOTE: single-waiter — callers must await each next() before calling again; for-await is the only safe usage. Concurrent next() calls would overwrite the pending waiter.
            next(): Promise<IteratorResult<unknown>> {
              if (state.queue.length)
                return Promise.resolve({ value: state.queue.shift(), done: false });
              if (state.error) return Promise.reject(state.error);
              if (state.done) return Promise.resolve({ value: undefined, done: true });
              return new Promise((resolve, reject) => {
                state.waiter = { resolve, reject };
              });
            },
          };
        },
      };
    },
  };

  function pushSub(s: SubState | undefined, payload: unknown) {
    if (!s) return;
    if (s.waiter) {
      const w = s.waiter;
      s.waiter = undefined;
      w.resolve({ value: payload, done: false });
    } else {
      s.queue.push(payload);
    }
  }

  function endSub(id: string, err?: Error) {
    const s = subs.get(id);
    if (!s) return;
    s.done = true;
    if (err) s.error = err;
    if (s.waiter) {
      const w = s.waiter;
      s.waiter = undefined;
      if (err) w.reject(err);
      else w.resolve({ value: undefined, done: true });
    }
    subs.delete(id);
  }
}
