import type * as net from "node:net";
import { bindServer } from "./bootstrap.ts";
import type { HandlerServerFrame, HandlerUdsEnvelope } from "./protocol.ts";
import {
  abortAllForConnection,
  handleSubCancel,
  handleSubOpen,
  writeServerFrame,
} from "./channels-server.ts";

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
        console.error("handlers-server: handleLine threw", err);
      });
    }
  });
  conn.on("close", () => abortAllForConnection(conn));
  // EPIPE / ECONNRESET on peer-close are delivered here; `close` handles cleanup.
  conn.on("error", () => {});
}

async function handleLine(conn: net.Socket, line: string): Promise<void> {
  let env: HandlerUdsEnvelope;
  try {
    env = JSON.parse(line) as HandlerUdsEnvelope;
  } catch {
    return;
  }
  const f = env.frame;
  if (f.kind === "sub.open") {
    handleSubOpen(conn, env.aid, f);
    return;
  }
  if (f.kind === "sub.cancel") {
    handleSubCancel(conn, env.aid, f);
    return;
  }
  if (f.kind !== "call") {
    return;
  }
  const handlers = activityHandlers.get(env.aid) ?? {};
  const fn = handlers[f.name];
  const resp: HandlerServerFrame = !fn
    ? { kind: "error", id: f.id, code: "UNKNOWN_HANDLER", message: f.name }
    : await invokeHandler(fn, f.id, f.payload);
  writeServerFrame(conn, env.aid, resp);
}

async function invokeHandler(
  fn: (req: never) => Promise<unknown>,
  id: string,
  payload: unknown,
): Promise<HandlerServerFrame> {
  try {
    const result = await fn(payload as never);
    return { kind: "result", id, payload: result };
  } catch (e) {
    return {
      kind: "error",
      id,
      code: "HANDLER_ERROR",
      message: e instanceof Error ? e.message : String(e),
    };
  }
}
