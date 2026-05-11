import {
  bootstrap,
  createActivity,
  createPane,
  splitPane,
  type ChannelMap,
  type ChannelCtx,
  type HandlerMap,
} from "@ozmux/sdk/server";
import { fileURLToPath } from "node:url";

interface MemoHandlers extends HandlerMap {
  greet: (req: { name: string }) => Promise<{ message: string }>;
}

interface MemoChannels extends ChannelMap {
  clock: (
    params: { intervalMs: number },
    ctx: ChannelCtx,
  ) => AsyncGenerator<{ time: string }>;
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const onAbort = () => {
      clearTimeout(t);
      resolve();
    };
    const t = setTimeout(() => {
      signal.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

bootstrap({
  commands: {
    memo: async (ctx) => {
      ctx.stdout.write(`memo invoked in pane ${ctx.pane.paneId}\n`);
      const activityId = await createActivity<MemoHandlers, MemoChannels>({
        html: fileURLToPath(new URL("./index.html", import.meta.url)),
        handlers: {
          greet: async ({ name }) => ({ message: `Hello, ${name}!` }),
        },
        channels: {
          clock: async function* ({ intervalMs }, { signal }) {
            yield { time: new Date().toISOString() };
            while (!signal.aborted) {
              await sleep(intervalMs, signal);
              if (signal.aborted) return;
              yield { time: new Date().toISOString() };
            }
          },
        },
      });
      const pane = await createPane({ activityId });
      await splitPane({
        target: ctx.pane.paneId,
        paneToPlace: pane,
        orientation: "vertical",
        side: "after",
      });
      return 0;
    },
  },
});
