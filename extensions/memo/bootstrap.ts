// extensions/memo/bootstrap.ts
import {
  bootstrap,
  createActivity,
  createPane,
  splitPane,
} from "@ozmux/sdk/server";
import { fileURLToPath } from "node:url";

bootstrap({
  commands: {
    memo: async (ctx) => {
      ctx.stdout.write(`memo invoked in pane ${ctx.pane.paneId}\n`);
      const activityId = await createActivity({
        html: fileURLToPath(new URL("./index.html", import.meta.url)),
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
