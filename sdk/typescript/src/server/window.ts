import { deleteNoContent, getJson, paths, postNoContent } from './daemon-client.ts';

export type WindowId = string;

/**
 * Client-side handle to a Window. The server holds the source of truth; this
 * class is a thin RPC façade. State fields (name, sessionId) are accepted at
 * construction so callers can build a Window from environment-supplied ids
 * without an extra round-trip.
 */
export class Window {
  readonly id: WindowId;
  readonly name: string;
  readonly sessionId: string | null;

  constructor(args: { id: WindowId; name: string; sessionId?: string | null }) {
    this.id = args.id;
    this.name = args.name;
    this.sessionId = args.sessionId ?? null;
  }

  /** Reify a Window class from its id by asking the daemon. */
  static async fetch(id: WindowId): Promise<Window> {
    const data = await getJson<{ window_id: string; name: string }>(paths.window(id));
    return new Window({ id: data.window_id, name: data.name });
  }

  async select(): Promise<void> {
    await postNoContent(paths.windowSelect(this.id), {});
  }

  async delete(): Promise<void> {
    await deleteNoContent(paths.window(this.id));
  }
}
