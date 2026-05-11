import type * as net from "node:net";
import { bindServer } from "./bootstrap.ts";
import type { HandlerServerFrame, HandlerUdsEnvelope } from "./protocol.ts";

export type HandlerMap = Record<string, (req: never) => Promise<unknown>>;
type ActivityId = string;

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

export function bindHandlersServer(sockPath: string): Promise<net.Server> {
  return bindServer(sockPath, onConnection, { maxConnections: 64 });
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
        // A malformed frame should not tear down the channel.
        console.error("handlers-server: handleLine threw", err);
      });
    }
  });
  // Node's default error handler would crash the process; the daemon closing
  // the UDS triggers EOF here, not a fatal error.
  conn.on("error", () => {});
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
