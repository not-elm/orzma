import * as path from "node:path";
import {
  type ChannelMap,
  registerActivityChannels,
} from "./channels-server.ts";
import { postJson } from "./daemon-client.ts";
import {
  type HandlerMap,
  registerActivityHandlers,
} from "./handlers-server.ts";

export type ActivityId = string;

export interface CreateActivityArgs<
  H extends HandlerMap = HandlerMap,
  C extends ChannelMap = ChannelMap,
> {
  html: string;
  handlers?: H;
  channels?: C;
}

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
