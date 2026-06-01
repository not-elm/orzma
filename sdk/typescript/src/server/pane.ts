import * as path from 'node:path';
import { Activity, type ActivityId, type ActivityKind } from './activity.ts';
import {
  type ChannelMap,
  registerActivityChannels,
  unregisterActivityChannels,
} from './channels-server.ts';
import { callControl, type SplitControlReply } from './control-client.ts';
import { deleteNoContent, paths, postJson, postNoContent } from './daemon-client.ts';
import {
  type HandlerMap,
  registerActivityHandlers,
  unregisterActivityHandlers,
} from './handlers-server.ts';

/**
 * The HTML entry path the webview loads, relative to the extension dir (the
 * asset root = `process.cwd()`), normalized to forward slashes so it is a
 * well-formed `ozmux-ext://<name>/<entry>` URL path on every OS.
 */
function toEntry(html: string): string {
  return path.relative(process.cwd(), html).split(path.sep).join('/');
}

export type PaneId = string;
export type WindowId = string;
export type SessionId = string;

export type Side = 'before' | 'after';
export type Orientation = 'horizontal' | 'vertical';

/**
 * What the caller hands to `Pane.split()` / `Pane.addActivity()`. The
 * terminal variant just creates a shell; the extension variant takes an HTML
 * entry path plus optional handlers/channels.
 */
export type ActivitySpecInput =
  | { kind: 'terminal'; name?: string }
  | {
      kind: 'extension';
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
   * Atomic split over the control socket. Primes local handler/channel
   * registries (consumed by the rendered activity in a later sub-project),
   * sends the split, and adopts the host-authoritative pane/activity ids for
   * the returned handle. On failure the local registries roll back.
   */
  async split(args: SplitArgs): Promise<Pane> {
    const activityId: ActivityId = crypto.randomUUID();
    primeActivityRegistries(activityId, args.activity);

    let reply: SplitControlReply;
    try {
      reply = await callControl('split', this.id, {
        side: args.side,
        orientation: args.orientation,
        activity: controlActivity(activityId, args.activity),
      });
    } catch (err) {
      rollbackActivityRegistries(activityId, args.activity);
      throw err;
    }

    return new Pane({
      id: reply.new_pane_id,
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

function primeActivityRegistries(activityId: ActivityId, spec: ActivitySpecInput): void {
  if (spec.kind !== 'extension') return;
  if (spec.handlers) registerActivityHandlers(activityId, spec.handlers);
  if (spec.channels) registerActivityChannels(activityId, spec.channels);
}

function rollbackActivityRegistries(activityId: ActivityId, spec: ActivitySpecInput): void {
  if (spec.kind !== 'extension') return;
  if (spec.handlers) unregisterActivityHandlers(activityId);
  if (spec.channels) unregisterActivityChannels(activityId);
}

function activityKindForSpec(spec: ActivitySpecInput): ActivityKind {
  if (spec.kind === 'terminal') return { type: 'terminal' };
  return { type: 'extension', entry: toEntry(spec.html) };
}

function controlActivity(
  activityId: ActivityId,
  spec: ActivitySpecInput,
): {
  kind: 'extension';
  entry: string;
  name?: string;
  activity_id: string;
  extension_name?: string;
} {
  // TODO: terminal splits over the control socket are not supported in #2/#3; a future op should carry a terminal kind instead of this extension fallback.
  if (spec.kind !== 'extension') {
    return { kind: 'extension', entry: '', name: spec.name, activity_id: activityId };
  }
  return {
    kind: 'extension',
    entry: toEntry(spec.html),
    name: spec.name,
    activity_id: activityId,
    extension_name: requireExtensionName(),
  };
}

function buildActivityPayload(
  activityId: ActivityId,
  spec: ActivitySpecInput,
): Record<string, unknown> {
  const base: Record<string, unknown> = { activity_id: activityId };
  if (spec.name !== undefined) base.name = spec.name;
  if (spec.kind === 'terminal') {
    base.kind = { type: 'terminal' };
  } else {
    // `extension_name` lets the daemon populate its ExtensionRegistry so
    // the in-CEF extension client can resolve the owning extension's UDS.
    // Resolved lazily from the env to match `daemon-client.ts`'s pattern; the
    // SDK is only ever used from inside a bootstrap()-driven extension process
    // where this is guaranteed to be set.
    base.kind = {
      type: 'extension',
      entry: toEntry(spec.html),
      extension_name: requireExtensionName(),
    };
  }
  return base;
}

// `EXTENSION_NAME` is set once by the bootstrap before any user code runs
// and never changes for the lifetime of the process. Cache the resolved value
// so we don't reach for `process.env` on every split / addActivity — the env
// object can be surprisingly slow under Node when accessed frequently.
let extensionNameCache: string | null = null;

function requireExtensionName(): string {
  if (extensionNameCache !== null) return extensionNameCache;
  const name = process.env.EXTENSION_NAME;
  if (!name) {
    throw new Error(
      'missing required env: EXTENSION_NAME (must be set by the SDK bootstrap before splitting / adding an extension activity)',
    );
  }
  extensionNameCache = name;
  return name;
}

/**
 * Test-only: drop the cached `EXTENSION_NAME`. Production code never calls
 * this — the env var is fixed at boot — but tests that mutate `process.env`
 * between assertions need a way to invalidate the cache.
 */
export function __resetExtensionNameCacheForTests(): void {
  extensionNameCache = null;
}
