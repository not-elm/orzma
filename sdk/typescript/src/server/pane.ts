import type { ActivityId } from "./activity.ts";
import { postJson, postNoContent } from "./daemon-client.ts";

export type PaneId = string;

export interface CreatePaneArgs {
  activityId: ActivityId;
}

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

export async function splitPane(args: SplitPaneArgs): Promise<void> {
  await postNoContent(`/panes/${args.target}/split-with`, {
    pane_id: args.paneToPlace,
    side: args.side,
    orientation: args.orientation,
  });
}
