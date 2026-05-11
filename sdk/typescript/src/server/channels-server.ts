import type * as net from "node:net";
import type {
  SubOpenFrame,
  SubCancelFrame,
  HandlerServerFrame,
} from "./protocol.ts";

type ActivityId = string;
type SubId = string;

export type ChannelCtx = { signal: AbortSignal };
export type ChannelGenerator<P = never, T = unknown> = (
  params: P,
  ctx: ChannelCtx,
) => AsyncGenerator<T, void, undefined>;
export type ChannelMap = Record<string, ChannelGenerator<any, any>>;

const activityChannels = new Map<ActivityId, ChannelMap>();

export function registerActivityChannels(
  aid: ActivityId,
  channels: ChannelMap,
): void {
  activityChannels.set(aid, channels);
}

/** Test-only escape hatch; not exported from the package barrel. */
export function __resetActivityChannelsForTests(): void {
  activityChannels.clear();
  for (const map of perConnection.values()) {
    for (const ac of map.values()) ac.abort();
    map.clear();
  }
  perConnection.clear();
}

// Per-connection subscription tracking. Sockets are removed explicitly on
// close via `abortAllForConnection`, so a regular Map is fine.
const perConnection = new Map<net.Socket, Map<SubId, AbortController>>();

function getSubs(conn: net.Socket): Map<SubId, AbortController> {
  let m = perConnection.get(conn);
  if (!m) {
    m = new Map();
    perConnection.set(conn, m);
  }
  return m;
}

function writeFrame(
  conn: net.Socket,
  aid: ActivityId,
  frame: HandlerServerFrame,
): void {
  if (conn.destroyed || !conn.writable) return;
  conn.write(JSON.stringify({ aid, frame }) + "\n");
}

export function handleSubOpen(
  conn: net.Socket,
  aid: ActivityId,
  open: SubOpenFrame,
): void {
  const channels = activityChannels.get(aid) ?? {};
  const gen = channels[open.name];
  if (!gen) {
    writeFrame(conn, aid, {
      kind: "sub.error",
      id: open.id,
      code: "UNKNOWN_CHANNEL",
      message: open.name,
    });
    return;
  }
  const ac = new AbortController();
  const subs = getSubs(conn);
  subs.set(open.id, ac);

  void (async () => {
    try {
      const iter = gen(open.params as never, { signal: ac.signal });
      for await (const value of iter) {
        if (ac.signal.aborted) break;
        writeFrame(conn, aid, {
          kind: "sub.data",
          id: open.id,
          payload: value,
        });
      }
      writeFrame(conn, aid, { kind: "sub.complete", id: open.id });
    } catch (e) {
      if (!ac.signal.aborted) {
        writeFrame(conn, aid, {
          kind: "sub.error",
          id: open.id,
          code: "HANDLER_ERROR",
          message: e instanceof Error ? e.message : String(e),
        });
      } else {
        // Cancelled-then-threw is a normal cancel; emit complete.
        writeFrame(conn, aid, { kind: "sub.complete", id: open.id });
      }
    } finally {
      subs.delete(open.id);
    }
  })();
}

export function handleSubCancel(
  conn: net.Socket,
  _aid: ActivityId,
  cancel: SubCancelFrame,
): void {
  const subs = perConnection.get(conn);
  const ac = subs?.get(cancel.id);
  if (ac) ac.abort();
}

export function abortAllForConnection(conn: net.Socket): void {
  const subs = perConnection.get(conn);
  if (!subs) return;
  for (const ac of subs.values()) ac.abort();
  subs.clear();
  perConnection.delete(conn);
}
