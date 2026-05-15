import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  // NOTE: managed dev server is Vite only. The daemon (:3200) must be running
  // separately — start `make dev-e2e` in another terminal. globalSetup pings
  // the daemon and gives an actionable error if it's not up.
  globalSetup: './e2e/global-setup.ts',
  webServer: {
    command: 'pnpm dev',
    url: 'http://localhost:5173',
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
  use: {
    baseURL: 'http://localhost:5173',
    // Clipboard tests need both permissions; Chromium honors these in headless.
    permissions: ['clipboard-read', 'clipboard-write'],
  },
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
});
