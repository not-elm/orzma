import * as path from "node:path";
import { postJson } from "./daemon-client.ts";
import {
  registerActivityHandlers,
  type HandlerMap,
} from "./handlers-server.ts";

export type ActivityId = string;

export interface CreateActivityArgs<H extends HandlerMap = HandlerMap> {
  html: string;
  handlers?: H;
}

export async function createActivity<H extends HandlerMap = HandlerMap>(
  args: CreateActivityArgs<H>,
): Promise<ActivityId> {
  const html = path.resolve(args.html);
  const { activity_id } = await postJson<{ activity_id: ActivityId }>(
    "/activities",
    { html },
  );
  if (args.handlers) {
    registerActivityHandlers(activity_id, args.handlers);
  }
  return activity_id;
}
