import { bootstrap, registerActivityChannels } from "@ozmux/sdk/server";

registerActivityChannels("fixture-aid-1", {
  ticks: async function* ({ n }, { signal }) {
    for (let i = 0; i < n; i++) {
      if (signal.aborted) return;
      yield { i };
    }
  },
});

await bootstrap({
  commands: {
    // No commands needed for this fixture — the e2e test connects to the
    // handlers UDS directly. bootstrap() is invoked so the SDK plumbing
    // (UDS listener etc.) is set up the same way as real extensions.
    "clock-ext": async () => 0,
  },
});
