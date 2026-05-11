import * as fs from "node:fs/promises";
import * as net from "node:net";
import type { HandlerUdsEnvelope, HandlerServerFrame } from "./protocol.ts";

export type HandlerMap = Record<
  string,
  (req: never) => Promise<unknown>
>;
type ActivityId = string;

// Per-process singleton. Populated by createActivity, consumed by the
// connection handler below. Cleared in tests via __resetActivityHandlersForTests.
const activityHandlers = new Map<ActivityId, HandlerMap>();

export function registerActivityHandlers(
  aid: ActivityId,
  handlers: HandlerMap,
): void {
  activityHandlers.set(aid, handlers);
}

/** Test-only escape hatch; not exported from the package barrel. */
export function __resetActivityHandlersForTests(): void {
  activityHandlers.clear();
}

export async function bindHandlersServer(
  sockPath: string,
): Promise<net.Server> {
  await fs.unlink(sockPath).catch(() => {});
  const server = net.createServer(onConnection);
  server.maxConnections = 64;
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(sockPath, () => {
      server.off("error", reject);
      resolve();
    });
  });
  await fs.chmod(sockPath, 0o600);
  return server;
}

function onConnection(conn: net.Socket): void {
  let buf = "";
  conn.on("data", (chunk) => {
    buf += chunk.toString("utf8");
    while (true) {
      const idx = buf.indexOf("\n");
      if (idx === -1) break;
      const line = buf.slice(0, idx);
      buf = buf.slice(idx + 1);
      handleLine(conn, line).catch((err) => {
        // structured failure: log and keep the connection
        // (a malformed frame should not kill the channel)
        // eslint-disable-next-line no-console
        console.error("handlers-server: handleLine threw", err);
      });
    }
  });
  conn.on("error", () => {
    // suppress; .end()/destroy are triggered by EOF on the daemon side
  });
}

async function handleLine(conn: net.Socket, line: string): Promise<void> {
  let env: HandlerUdsEnvelope;
  try {
    env = JSON.parse(line) as HandlerUdsEnvelope;
  } catch {
    return; // ignore non-JSON noise
  }
  if (env.frame.kind !== "call") {
    return; // only "call" is expected inbound on this side
  }
  const call = env.frame;
  const handlers = activityHandlers.get(env.aid) ?? {};
  const fn = handlers[call.name];
  let resp: HandlerServerFrame;
  if (!fn) {
    resp = {
      kind: "error",
      id: call.id,
      code: "UNKNOWN_HANDLER",
      message: call.name,
    };
  } else {
    try {
      const result = await fn(call.payload as never);
      resp = { kind: "result", id: call.id, payload: result };
    } catch (e) {
      resp = {
        kind: "error",
        id: call.id,
        code: "HANDLER_ERROR",
        message: e instanceof Error ? e.message : String(e),
      };
    }
  }
  conn.write(JSON.stringify({ aid: env.aid, frame: resp }) + "\n");
}
