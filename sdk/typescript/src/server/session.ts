import { deleteNoContent, getJson, paths } from "./daemon-client.ts";
import type { WindowId } from "./window.ts";

export type SessionId = string;

/**
 * Client-side handle to a Session. Like `Window`, this is a thin RPC façade;
 * fields are accepted at construction so callers can wrap an env-supplied id
 * without a round-trip. Use `Session.fetch(id)` when you need authoritative
 * state.
 */
export class Session {
  readonly id: SessionId;
  readonly name: string;
  readonly linkedWindowIds: WindowId[];
  readonly activeWindowId: WindowId | null;

  constructor(args: {
    id: SessionId;
    name: string;
    linkedWindowIds?: WindowId[];
    activeWindowId?: WindowId | null;
  }) {
    this.id = args.id;
    this.name = args.name;
    this.linkedWindowIds = args.linkedWindowIds ?? [];
    this.activeWindowId = args.activeWindowId ?? null;
  }

  static async fetch(id: SessionId): Promise<Session> {
    const data = await getJson<{
      session_id: string;
      name: string;
      linked_windows: string[];
      active_window: string | null;
    }>(paths.session(id));
    return new Session({
      id: data.session_id,
      name: data.name,
      linkedWindowIds: data.linked_windows,
      activeWindowId: data.active_window,
    });
  }

  async delete(): Promise<void> {
    await deleteNoContent(paths.session(this.id));
  }
}
