import * as net from 'node:net';

/** Parameters for a `split` control call. */
export interface SplitControlParams {
  side: 'before' | 'after';
  orientation: 'horizontal' | 'vertical';
  activity:
    | { kind: 'extension'; entry: string; name?: string | null; activity_id: string }
    | { kind: 'browser'; url: string; name?: string | null; activity_id: string };
}

/** Parameters for an `add_activity` control call. */
export interface AddActivityControlParams {
  activity:
    | { kind: 'extension'; entry: string; name?: string | null; activity_id: string }
    | { kind: 'browser'; url: string; name?: string | null; activity_id: string };
}

/** Parameters for an `activate` control call. */
export interface ActivateControlParams {
  activity_id: string;
}

type ControlParamsByOp = {
  split: SplitControlParams;
  add_activity: AddActivityControlParams;
  activate: ActivateControlParams;
};

type ControlReplyByOp = {
  split: { new_pane_id: string; new_activity_id: string };
  add_activity: { new_activity_id: string };
  activate: Record<string, never>;
};

const SYNTHETIC_REPLY: { [K in keyof ControlReplyByOp]: () => ControlReplyByOp[K] } = {
  split: () => ({ new_pane_id: crypto.randomUUID(), new_activity_id: crypto.randomUUID() }),
  add_activity: () => ({ new_activity_id: crypto.randomUUID() }),
  activate: () => ({}) satisfies Record<string, never>,
};

const CONNECT_TIMEOUT_MS = 5000;

/**
 * Sends one `call` frame over the control UDS and resolves with the `result`
 * payload, or rejects on an `error` frame / connection failure. When
 * `OZMUX_CONTROL_SOCK_PATH` is unset, resolves with a synthetic op-appropriate
 * reply (no-op) so a bare `node bootstrap.ts` and host-less tests still pass.
 */
export function callControl<Op extends keyof ControlParamsByOp>(
  op: Op,
  pane: string,
  params: ControlParamsByOp[Op],
): Promise<ControlReplyByOp[Op]> {
  const sockPath = process.env.OZMUX_CONTROL_SOCK_PATH;
  if (!sockPath) {
    console.warn(`ozmux: OZMUX_CONTROL_SOCK_PATH unset — skipping ${op} (no-op)`);
    return Promise.resolve(SYNTHETIC_REPLY[op]());
  }

  return new Promise((resolve, reject) => {
    const id = crypto.randomUUID();
    const sock = net.connect(sockPath);
    let buffer = '';
    let settled = false;

    let timer: ReturnType<typeof setTimeout>;
    const fail = (err: Error) => {
      clearTimeout(timer);
      if (settled) return;
      settled = true;
      sock.destroy();
      reject(err);
    };
    timer = setTimeout(
      () => fail(new Error(`ozmux: control connect timeout after ${CONNECT_TIMEOUT_MS}ms`)),
      CONNECT_TIMEOUT_MS,
    );

    sock.once('connect', () => {
      clearTimeout(timer);
      sock.write(`${JSON.stringify({ kind: 'call', id, op, pane, params })}\n`);
    });
    sock.on('data', (chunk: Buffer) => {
      buffer += chunk.toString('utf8');
      const nl = buffer.indexOf('\n');
      if (nl < 0) return;
      const line = buffer.slice(0, nl);
      let frame: {
        kind?: string;
        payload?: ControlReplyByOp[Op];
        code?: string;
        message?: string;
      };
      try {
        frame = JSON.parse(line);
      } catch {
        fail(new Error('ozmux: malformed control response'));
        return;
      }
      if (settled) return;
      settled = true;
      sock.destroy();
      if (frame.kind === 'result') {
        // A `result` frame must carry a payload (`{}` for ops with no data,
        // e.g. activate). Treat an absent/null payload as a malformed response
        // rather than resolving `undefined`, which would crash callers that
        // read `reply.new_*_id`.
        if (frame.payload == null) {
          reject(new Error('ozmux: malformed control result (missing payload)'));
          return;
        }
        resolve(frame.payload as ControlReplyByOp[Op]);
      } else {
        reject(new Error(`ozmux: control ${frame.code ?? 'error'}: ${frame.message ?? ''}`));
      }
    });
    sock.on('error', (e) => fail(e instanceof Error ? e : new Error(String(e))));
    sock.on('close', () => fail(new Error('ozmux: control socket closed before response')));
  });
}
