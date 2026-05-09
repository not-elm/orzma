import { bootstrap } from "@ozmux/sdk/server";

await bootstrap({
  commands: {
    crashext: async () => 0,
  },
});
// Simulate a crash *after* shim materialization.
setTimeout(() => process.exit(7), 200);
