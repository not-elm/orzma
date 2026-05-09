// extensions/memo/bootstrap.ts
import { bootstrap, createActivity } from "@ozmux/sdk/server";
import path from "path";

bootstrap({
  commands: {
    memo: async (ctx) => {
      ctx.stdout.write(`memo invoked in pane ${ctx.pane.paneId}\n`);
      await createActivity({
        html: path.join(__dirname, "index.html"),
        initialData: ctx.argv[0],
      });
      return 0;
    },
  },
});
