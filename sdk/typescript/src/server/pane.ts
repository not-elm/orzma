import * as path from 'node:path';
import {
  type ChannelMap,
  registerSurfaceChannels,
  unregisterSurfaceChannels,
} from './channels-server.ts';
import { callControl } from './control-client.ts';
import { deleteNoContent, paths, postNoContent } from './daemon-client.ts';
import {
  type HandlerMap,
  registerSurfaceHandlers,
  unregisterSurfaceHandlers,
} from './handlers-server.ts';
import { Surface, type SurfaceId, type SurfaceKind } from './surface.ts';

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
 * What the caller hands to `Pane.split()` / `Pane.addSurface()`. The
 * terminal variant just creates a shell; the extension variant takes an HTML
 * entry path plus optional handlers/channels; the browser variant opens a URL
 * directly in the embedded webview without binding to any extension.
 */
export type SurfaceSpecInput =
  | { kind: 'terminal'; name?: string }
  | {
      kind: 'extension';
      html: string;
      name?: string;
      handlers?: HandlerMap;
      channels?: ChannelMap;
    }
  | { kind: 'browser'; url: string; name?: string };

export interface SplitArgs {
  side: Side;
  orientation: Orientation;
  surface: SurfaceSpecInput;
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
   * registries (consumed by the rendered surface in a later sub-project),
   * sends the split, and adopts the host-authoritative pane/surface ids for
   * the returned handle. On failure the local registries roll back.
   */
  async split(args: SplitArgs): Promise<Pane> {
    const surfaceId: SurfaceId = crypto.randomUUID();
    primeSurfaceRegistries(surfaceId, args.surface);

    let reply: { new_pane_id: string; new_surface_id: string };
    try {
      reply = await callControl('split', this.id, {
        side: args.side,
        orientation: args.orientation,
        surface: controlSurface(surfaceId, args.surface),
      });
    } catch (err) {
      rollbackSurfaceRegistries(surfaceId, args.surface);
      throw err;
    }

    return new Pane({
      id: reply.new_pane_id,
      windowId: this.windowId,
      sessionId: this.sessionId,
    });
  }

  /**
   * Adds a new Surface (tab) to this Pane without splitting. Primes local
   * handler/channel registries before the call and adopts the host-authoritative
   * surface id for the returned handle. On failure the local registries roll back.
   */
  async addSurface(spec: SurfaceSpecInput): Promise<Surface> {
    const surfaceId: SurfaceId = crypto.randomUUID();
    primeSurfaceRegistries(surfaceId, spec);

    let reply: { new_surface_id: string };
    try {
      reply = await callControl('add_surface', this.id, {
        surface: controlSurface(surfaceId, spec),
      });
    } catch (err) {
      rollbackSurfaceRegistries(surfaceId, spec);
      throw err;
    }

    return new Surface({
      id: reply.new_surface_id,
      paneId: this.id,
      windowId: this.windowId,
      sessionId: this.sessionId,
      kind: surfaceKindForSpec(spec),
    });
  }

  async activate(): Promise<void> {
    await postNoContent(paths.paneActivate(this.windowId, this.id), {});
  }

  async close(): Promise<void> {
    await deleteNoContent(paths.pane(this.windowId, this.id));
  }
}

function primeSurfaceRegistries(surfaceId: SurfaceId, spec: SurfaceSpecInput): void {
  if (spec.kind !== 'extension') return;
  if (spec.handlers) registerSurfaceHandlers(surfaceId, spec.handlers);
  if (spec.channels) registerSurfaceChannels(surfaceId, spec.channels);
}

function rollbackSurfaceRegistries(surfaceId: SurfaceId, spec: SurfaceSpecInput): void {
  if (spec.kind !== 'extension') return;
  if (spec.handlers) unregisterSurfaceHandlers(surfaceId);
  if (spec.channels) unregisterSurfaceChannels(surfaceId);
}

function surfaceKindForSpec(spec: SurfaceSpecInput): SurfaceKind {
  if (spec.kind === 'terminal') return { type: 'terminal' };
  if (spec.kind === 'browser') return { type: 'browser', initial_url: spec.url };
  return { type: 'extension', entry: toEntry(spec.html) };
}

function controlSurface(
  surfaceId: SurfaceId,
  spec: SurfaceSpecInput,
):
  | {
      kind: 'extension';
      entry: string;
      name?: string;
      surface_id: string;
      extension_name?: string;
    }
  | { kind: 'browser'; url: string; name?: string; surface_id: string } {
  if (spec.kind === 'browser') {
    return { kind: 'browser', url: spec.url, name: spec.name, surface_id: surfaceId };
  }
  // TODO: terminal splits over the control socket are not supported in #2/#3; a future op should carry a terminal kind instead of this extension fallback.
  if (spec.kind !== 'extension') {
    return { kind: 'extension', entry: '', name: spec.name, surface_id: surfaceId };
  }
  return {
    kind: 'extension',
    entry: toEntry(spec.html),
    name: spec.name,
    surface_id: surfaceId,
    extension_name: requireExtensionName(),
  };
}

// `EXTENSION_NAME` is set once by the bootstrap before any user code runs
// and never changes for the lifetime of the process. Cache the resolved value
// so we don't reach for `process.env` on every split / addSurface — the env
// object can be surprisingly slow under Node when accessed frequently.
let extensionNameCache: string | null = null;

function requireExtensionName(): string {
  if (extensionNameCache !== null) return extensionNameCache;
  const name = process.env.EXTENSION_NAME;
  if (!name) {
    throw new Error(
      'missing required env: EXTENSION_NAME (must be set by the SDK bootstrap before splitting / adding an extension surface)',
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
