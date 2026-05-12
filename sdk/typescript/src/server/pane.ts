import * as path from "node:path";
import {
  type ChannelMap,
  registerActivityChannels,
  unregisterActivityChannels,
} from "./channels-server.ts";
import {
  deleteNoContent,
  paths,
  postJson,
  postNoContent,
} from "./daemon-client.ts";
import {
  type HandlerMap,
  registerActivityHandlers,
  unregisterActivityHandlers,
} from "./handlers-server.ts";
import { Activity, type ActivityId, type ActivityKind } from "./activity.ts";

export type PaneId = string;
export type WindowId = string;
export type SessionId = string;

export type Side = "before" | "after";
export type Orientation = "horizontal" | "vertical";

/**
 * What the caller hands to `Pane.split()` / `Pane.addActivity()`. The
 * terminal variant just creates a shell; the extension variant takes an HTML
 * entry path plus optional handlers/channels.
 */
export type ActivitySpecInput =
  | { kind: "terminal"; name?: string }
  | {
      kind: "extension";
      html: string;
      name?: string;
      handlers?: HandlerMap;
      channels?: ChannelMap;
    };

export interface SplitArgs {
  side: Side;
  orientation: Orientation;
  activity: ActivitySpecInput;
}

/**
 * Lightweight client-side handle to a Pane. Holds the addressing tuple and
 * exposes mutating actions. No state is fetched eagerly — the caller already
 * knows the ids (env vars or upstream call result).
 */
export class Pane {
  readonly id: PaneId;
  readonly windowId: WindowId;
  readonly sessionId: SessionId | null;

  constructor(args: {
    id: PaneId;
    windowId: WindowId;
    sessionId?: SessionId | null;
  }) {
    this.id = args.id;
    this.windowId = args.windowId;
    this.sessionId = args.sessionId ?? null;
  }

  /**
   * Atomic split: generate UUIDs, prime local handler/channel registries,
   * then POST. The pre-POST register is what makes the iframe race-free —
   * see plan §5.3 "race-free invariant". On POST failure we roll the local
   * registries back so no stale entries survive.
   */
  async split(args: SplitArgs): Promise<Pane> {
    const newPaneId: PaneId = crypto.randomUUID();
    const activityId: ActivityId = crypto.randomUUID();

    primeActivityRegistries(activityId, args.activity);

    const body = {
      side: args.side,
      orientation: args.orientation,
      new_pane_id: newPaneId,
      activity: buildActivityPayload(activityId, args.activity),
    };

    try {
      await postJson(paths.paneSplit(this.windowId, this.id), body);
    } catch (err) {
      rollbackActivityRegistries(activityId, args.activity);
      throw err;
    }

    return new Pane({
      id: newPaneId,
      windowId: this.windowId,
      sessionId: this.sessionId,
    });
  }

  /**
   * Add a new Activity (tab) to this Pane without splitting. Same
   * register-before-POST discipline as `split()`.
   */
  async addActivity(spec: ActivitySpecInput): Promise<Activity> {
    const activityId: ActivityId = crypto.randomUUID();
    primeActivityRegistries(activityId, spec);

    try {
      await postJson(paths.paneActivities(this.windowId, this.id), {
        activity: buildActivityPayload(activityId, spec),
      });
    } catch (err) {
      rollbackActivityRegistries(activityId, spec);
      throw err;
    }

    return new Activity({
      id: activityId,
      paneId: this.id,
      windowId: this.windowId,
      sessionId: this.sessionId,
      kind: activityKindForSpec(spec),
    });
  }

  async activate(): Promise<void> {
    await postNoContent(paths.paneActivate(this.windowId, this.id), {});
  }

  async close(): Promise<void> {
    await deleteNoContent(paths.pane(this.windowId, this.id));
  }
}

function primeActivityRegistries(
  activityId: ActivityId,
  spec: ActivitySpecInput,
): void {
  if (spec.kind !== "extension") return;
  if (spec.handlers) registerActivityHandlers(activityId, spec.handlers);
  if (spec.channels) registerActivityChannels(activityId, spec.channels);
}

function rollbackActivityRegistries(
  activityId: ActivityId,
  spec: ActivitySpecInput,
): void {
  if (spec.kind !== "extension") return;
  if (spec.handlers) unregisterActivityHandlers(activityId);
  if (spec.channels) unregisterActivityChannels(activityId);
}

function activityKindForSpec(spec: ActivitySpecInput): ActivityKind {
  if (spec.kind === "terminal") return { type: "terminal" };
  return { type: "extension", html_root: path.dirname(spec.html) };
}

function buildActivityPayload(
  activityId: ActivityId,
  spec: ActivitySpecInput,
): Record<string, unknown> {
  const base: Record<string, unknown> = { activity_id: activityId };
  if (spec.name !== undefined) base.name = spec.name;
  if (spec.kind === "terminal") {
    base.kind = { type: "terminal" };
  } else {
    // `extension_name` lets the daemon populate its ExtensionRegistry so the
    // iframe's handlers-WS upgrade can resolve the owning extension's UDS.
    // Resolved lazily from the env to match `daemon-client.ts`'s pattern; the
    // SDK is only ever used from inside a bootstrap()-driven extension process
    // where this is guaranteed to be set.
    base.kind = {
      type: "extension",
      html_root: path.dirname(spec.html),
      extension_name: requireExtensionName(),
    };
  }
  return base;
}

function requireExtensionName(): string {
  const name = process.env.EXTENSION_NAME;
  if (!name) {
    throw new Error(
      "missing required env: EXTENSION_NAME (must be set by the SDK bootstrap before splitting / adding an extension activity)",
    );
  }
  return name;
}
