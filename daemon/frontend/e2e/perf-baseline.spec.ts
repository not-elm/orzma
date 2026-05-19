import * as fs from 'node:fs/promises';
import { test } from '@playwright/test';

test('extract __OZMUX_PERF_REPORT to disk', async ({ page }) => {
  const out = process.env.OZMUX_PERF_REPORT_OUT;
  if (!out) throw new Error('OZMUX_PERF_REPORT_OUT env required');

  await page.goto('http://localhost:5173/?replay=synthetic_scroll_burst&record-perf=1');
  await page.waitForFunction(() => typeof window.__OZMUX_PERF_REPORT === 'function');
  await page.waitForFunction(() => window.__OZMUX_PERF_REPORT().total_marks >= 50, undefined, {
    timeout: 30_000,
  });
  const report = await page.evaluate(() => window.__OZMUX_PERF_REPORT({ includeRaw: false }));
  await fs.writeFile(out, JSON.stringify(report, null, 2));
});
