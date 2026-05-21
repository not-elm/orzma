import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { bootstrap } from "@ozmux/sdk/server";

await bootstrap({
  commands: {
    crashext: async () => 0,
  },
});
writeFileSync(join(process.env.OZMUX_BIN_DIR, "pid"), String(process.pid));
// Simulate a crash *after* shim materialization.
setTimeout(() => process.exit(7), 200);
