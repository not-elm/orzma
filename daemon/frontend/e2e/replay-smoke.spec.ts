import { expect, test } from '@playwright/test';

test('?replay=synthetic_scroll_burst&record-perf=1 fills perf buffer', async ({ page }) => {
  await page.goto('/?replay=synthetic_scroll_burst&record-perf=1');
  await page.waitForSelector('.terminal-grid', { timeout: 10_000 });

  // Wait for frames to arrive and be processed through the markStage chain.
  await page.waitForTimeout(2000);

  const bufferLength = await page.evaluate(() => {
    // biome-ignore lint/suspicious/noExplicitAny: globalThis perf buffer is injected by the runtime hook
    return ((globalThis as any).__ozmuxPerfBuffer?.writeIndex as number | undefined) ?? 0;
  });

  expect(bufferLength).toBeGreaterThan(0);
});
