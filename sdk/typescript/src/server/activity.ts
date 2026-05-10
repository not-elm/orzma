import * as path from "node:path";
import { postJson } from "./daemon-client.ts";

export type ActivityId = string;

export interface CreateActivityArgs {
  html: string;
}

export async function createActivity(
  args: CreateActivityArgs,
): Promise<ActivityId> {
  const html = path.resolve(args.html);
  const { activity_id } = await postJson<{ activity_id: ActivityId }>(
    "/activities",
    { html },
  );
  return activity_id;
}
