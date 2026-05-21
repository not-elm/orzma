import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { bootstrap } from "@ozmux/sdk/server";

await bootstrap({
  commands: {
    echoext: async (ctx) => {
      ctx.stdout.write(`PANE=${ctx.pane.paneId}\n`);
      ctx.stdout.write(`ARGV=${ctx.argv.join(",")}\n`);
      return 0;
    },
  },
});
writeFileSync(join(process.env.OZMUX_BIN_DIR, "pid"), String(process.pid));
