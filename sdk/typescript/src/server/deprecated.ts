/**
 * Legacy SDK surface kept for PR5 backward compat. These wrappers continue to
 * speak the flat HTTP routes from PR4 so existing extensions keep working
 * while they migrate to the class-based API. The whole file is deleted in
 * PR7.
 */
import * as path from "node:path";
import type { ActivityId } from "./activity.ts";
import {
  type ChannelMap,
  registerActivityChannels,
} from "./channels-server.ts";
import { postJson, postNoContent } from "./daemon-client.ts";
import {
  type HandlerMap,
  registerActivityHandlers,
} from "./handlers-server.ts";
import type { PaneId } from "./pane.ts";

export interface CreateActivityArgs<
  H extends HandlerMap = HandlerMap,
  C extends ChannelMap = ChannelMap,
> {
  html: string;
  handlers?: H;
  channels?: C;
}

/**
 * @deprecated Use `pane.split({ activity: { kind: "extension", html, ... } })`
 * or `pane.addActivity(...)`. Removed in PR7.
 */
export async function createActivity<
  H extends HandlerMap = HandlerMap,
  C extends ChannelMap = ChannelMap,
>(args: CreateActivityArgs<H, C>): Promise<ActivityId> {
  const html = path.resolve(args.html);
  const { activity_id } = await postJson<{ activity_id: ActivityId }>(
    "/activities",
    { html },
  );
  if (args.handlers) {
    registerActivityHandlers(activity_id, args.handlers);
  }
  if (args.channels) {
    registerActivityChannels(activity_id, args.channels);
  }
  return activity_id;
}

export interface CreatePaneArgs {
  activityId: ActivityId;
}

/**
 * @deprecated The limbo concept is going away. Use `pane.split({...})`
 * which atomically allocates pane + activity. Removed in PR7.
 */
export async function createPane(args: CreatePaneArgs): Promise<PaneId> {
  const { pane_id } = await postJson<{ pane_id: PaneId }>("/panes", {
    activity_id: args.activityId,
  });
  return pane_id;
}

export interface SplitPaneArgs {
  target: PaneId;
  paneToPlace: PaneId;
  orientation: "horizontal" | "vertical";
  side: "before" | "after";
}

/**
 * @deprecated Use `pane.split({...})` for an atomic split that does not
 * require a limbo pane. Removed in PR7.
 */
export async function splitPane(args: SplitPaneArgs): Promise<void> {
  await postNoContent(`/panes/${args.target}/split-with`, {
    pane_id: args.paneToPlace,
    side: args.side,
    orientation: args.orientation,
  });
}
