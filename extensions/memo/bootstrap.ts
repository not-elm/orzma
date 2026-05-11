// extensions/memo/bootstrap.ts
import {
  bootstrap,
  createActivity,
  createPane,
  splitPane,
  type HandlerMap,
} from "@ozmux/sdk/server";
import { fileURLToPath } from "node:url";

interface MemoHandlers extends HandlerMap {
  greet: (req: { name: string }) => Promise<{ message: string }>;
}

bootstrap({
  commands: {
    memo: async (ctx) => {
      ctx.stdout.write(`memo invoked in pane ${ctx.pane.paneId}\n`);
      const activityId = await createActivity<MemoHandlers>({
        html: fileURLToPath(new URL("./index.html", import.meta.url)),
        handlers: {
          greet: async ({ name }) => ({ message: `Hello, ${name}!` }),
        },
      });
      const pane = await createPane({
        activityId,
      });
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
