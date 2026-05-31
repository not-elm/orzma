import * as net from 'node:net';

/** Parameters for a `split` control call. */
export interface SplitControlParams {
  side: 'before' | 'after';
  orientation: 'horizontal' | 'vertical';
  activity: { kind: 'extension'; html_root: string; name?: string | null; activity_id: string };
}

/** The host's reply to a successful split. */
export interface SplitControlReply {
  new_pane_id: string;
  new_activity_id: string;
}

const CONNECT_TIMEOUT_MS = 5000;

/**
 * Sends one `call` frame over the control UDS and resolves with the `result`
 * payload (or rejects on an `error` frame / connection failure). When
 * `OZMUX_CONTROL_SOCK_PATH` is unset, resolves with synthetic ids (no-op) so a
 * bare `node bootstrap.ts` and host-less tests still pass.
 */
export function callControl(
  op: 'split',
  pane: string,
  params: SplitControlParams,
): Promise<SplitControlReply> {
  const sockPath = process.env.OZMUX_CONTROL_SOCK_PATH;
  if (!sockPath) {
    console.warn(`ozmux: OZMUX_CONTROL_SOCK_PATH unset — skipping ${op} (no-op)`);
    return Promise.resolve({
      new_pane_id: crypto.randomUUID(),
      new_activity_id: crypto.randomUUID(),
    });
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
      let frame: { kind?: string; payload?: SplitControlReply; code?: string; message?: string };
      try {
        frame = JSON.parse(line);
      } catch {
        fail(new Error('ozmux: malformed control response'));
        return;
      }
      if (settled) return;
      settled = true;
      sock.destroy();
      if (frame.kind === 'result' && frame.payload) resolve(frame.payload);
      else reject(new Error(`ozmux: control ${frame.code ?? 'error'}: ${frame.message ?? ''}`));
    });
    sock.on('error', (e) => fail(e instanceof Error ? e : new Error(String(e))));
    sock.on('close', () => fail(new Error('ozmux: control socket closed before response')));
  });
}
