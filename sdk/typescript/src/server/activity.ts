import { paths, postNoContent } from "./daemon-client.ts";

export type ActivityId = string;

export type ActivityKind =
  | { type: "terminal" }
  | { type: "extension"; html_root: string; extension_name?: string }
  | { type: "browser"; initial_url?: string };

/**
 * Lightweight client-side handle to an Activity. Carries the addressing tuple
 * needed to call hierarchical endpoints (`window → pane → activity`).
 * Construction is cheap — there is no server round-trip until a method is
 * invoked.
 */
export class Activity {
  readonly id: ActivityId;
  readonly paneId: string;
  readonly windowId: string;
  readonly sessionId: string | null;
  readonly kind: ActivityKind;

  constructor(args: {
    id: ActivityId;
    paneId: string;
    windowId: string;
    sessionId?: string | null;
    kind: ActivityKind;
  }) {
    this.id = args.id;
    this.paneId = args.paneId;
    this.windowId = args.windowId;
    this.sessionId = args.sessionId ?? null;
    this.kind = args.kind;
  }

  async activate(): Promise<void> {
    await postNoContent(
      paths.activityActivate(this.windowId, this.paneId, this.id),
      {},
    );
  }
}
