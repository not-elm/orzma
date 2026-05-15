import { request as playwrightRequest } from '@playwright/test';

const DAEMON_URL = 'http://localhost:3200';

export default async function globalSetup(): Promise<void> {
  const ctx = await playwrightRequest.newContext();
  try {
    await ctx.get(`${DAEMON_URL}/sessions`, { timeout: 2_000 });
  } catch (err) {
    throw new Error(
      `Phase 3A e2e: daemon not reachable at ${DAEMON_URL}. ` +
        'Run `make dev-e2e` in another terminal first, then re-run `pnpm test:e2e`. ' +
        `(${err instanceof Error ? err.message : String(err)})`,
    );
  } finally {
    await ctx.dispose();
  }
}
