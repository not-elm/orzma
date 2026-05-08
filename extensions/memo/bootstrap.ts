// extensions/memo/bootstrap.ts
import { bootstrap } from "@ozmux/sdk/server";
import { memoActivity } from "./activities/memo.ts";

bootstrap({
  commands: {
    memo: async (ctx) => {
      console.log("DDADAD");
      ctx.stdout.write(`memo invoked in pane ${ctx.pane.paneId}\n`);
      memoActivity.craete({});
      return 0;
    },
  },
});
