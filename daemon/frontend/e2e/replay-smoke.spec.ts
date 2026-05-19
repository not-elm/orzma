import { expect, test } from '@playwright/test';

test('?replay= first WS message is a hello frame', async ({ page }) => {
  const firstMessage = new Promise<string>((resolve) => {
    page.on('websocket', (ws) => {
      ws.on('framereceived', (frame) => {
        if (typeof frame.payload === 'string') {
          resolve(frame.payload);
        }
      });
    });
  });

  await page.goto('/?replay=synthetic_scroll_burst&record-perf=1');

  const raw = await Promise.race([
    firstMessage,
    new Promise<never>((_, reject) =>
      setTimeout(() => reject(new Error('timed out waiting for hello')), 10_000),
    ),
  ]);

  const parsed = JSON.parse(raw) as Record<string, unknown>;
  expect(parsed['kind']).toBe('hello');
  expect(typeof parsed['cols']).toBe('number');
  expect(typeof parsed['rows']).toBe('number');
  expect(typeof parsed['cursor']).toBe('object');
  expect(Array.isArray(parsed['escape_caps'])).toBe(true);
  expect(Array.isArray(parsed['input_caps'])).toBe(true);
  expect('bridge_started_at_unix_us' in parsed).toBe(true);
});

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
