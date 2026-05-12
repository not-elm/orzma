export { Session, type SessionId } from "./session.ts";
export { Window, type WindowId } from "./window.ts";
export {
  Pane,
  type PaneId,
  type Side,
  type Orientation,
  type ActivitySpecInput,
  type SplitArgs,
} from "./pane.ts";
export { Activity, type ActivityId, type ActivityKind } from "./activity.ts";
export {
  bootstrap,
  type CommandContext,
  type CommandHandler,
} from "./bootstrap.ts";
export { abortableSleep } from "./timing.ts";
export type { HandlerMap } from "./handlers-server.ts";
export {
  type ChannelCtx,
  type ChannelGenerator,
  type ChannelMap,
  registerActivityChannels,
} from "./channels-server.ts";

// Deprecated legacy API — removed in PR7. Kept here so existing extensions
// keep building while they migrate to the class-based surface.
export {
  createActivity,
  type CreateActivityArgs,
  createPane,
  type CreatePaneArgs,
  splitPane,
  type SplitPaneArgs,
} from "./deprecated.ts";
