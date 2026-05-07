import { bootstrap } from "@ozmux/sdk/server";
import { memoActivity } from "./activities/memo.ts";

bootstrap({
  commands: {
    memo: async ({ pane }) => {
      await memoActivity.craete({});
    },
  },
});
