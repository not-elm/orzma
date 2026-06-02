import { callControl } from './control-client.ts';

export type SurfaceId = string;

export type SurfaceKind =
  | { type: 'terminal' }
  | { type: 'extension'; entry: string; extension_name?: string }
  | { type: 'browser'; initial_url?: string };

/**
 * Lightweight client-side handle to a Surface. Carries the addressing tuple
 * needed to call hierarchical endpoints (`window → pane → surface`).
 * Construction is cheap — there is no server round-trip until a method is
 * invoked.
 */
export class Surface {
  readonly id: SurfaceId;
  readonly paneId: string;
  readonly windowId: string;
  readonly sessionId: string | null;
  readonly kind: SurfaceKind;

  constructor(args: {
    id: SurfaceId;
    paneId: string;
    windowId: string;
    sessionId?: string | null;
    kind: SurfaceKind;
  }) {
    this.id = args.id;
    this.paneId = args.paneId;
    this.windowId = args.windowId;
    this.sessionId = args.sessionId ?? null;
    this.kind = args.kind;
  }

  async activate(): Promise<void> {
    await callControl('activate', this.paneId, { surface_id: this.id });
  }
}
