// extensions/memo/bootstrap.ts
import { bootstrap, createActivity, createPane, splitPane } from "@ozmux/sdk/server";
import path from "path";

bootstrap({
  commands: {
    memo: async (ctx) => {
      ctx.stdout.write(`memo invoked in pane ${ctx.pane.paneId}\n`);
      const activityId = await createActivity({
        html: path.join(__dirname, "index.html"),
      });
      const pane = await createPane({
        activityId,
      });
      await splitPane({
        target: ctx.pane.paneId,
        paneToPlace: pane,
        orientation: "horizontal",
        side: "after",
      });
      return 0;
    },
  },
});
