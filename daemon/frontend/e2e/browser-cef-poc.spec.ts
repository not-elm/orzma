// PoC e2e for the cef screencast pipeline.
//
// Gated by OZMUX_TEST_REAL_CEF=1 because the test launches a real cef_host
// child via the daemon and exercises the WebGPU renderer end-to-end. The
// matching prerequisites — built cef_host binary, CEF framework on disk,
// daemon running on :3200 — are documented in CLAUDE.md alongside
// `make dev-e2e`.

import { expect, test } from '@playwright/test';

const HAS_CEF = process.platform === 'darwin' && process.env.OZMUX_TEST_REAL_CEF === '1';
const DAEMON = 'http://localhost:3200';

test.describe('CEF PoC e2e', () => {
  test.skip(!HAS_CEF, 'OZMUX_TEST_REAL_CEF=1 required (launches real CEF cef_host)');

  test('?cef=1 mounts BrowserActivityCef canvas', async ({ page }) => {
    await page.goto('/?cef=1');
    // Wait for any canvas to appear; BrowserActivityCef renders one immediately.
    await page.waitForSelector('canvas', { timeout: 10_000 });
    const count = await page.locator('canvas').count();
    expect(count).toBeGreaterThan(0);
  });

  test('PoC KPI smoke — 10 simulated wheel events, p95 < 500ms', async ({ page }) => {
    await page.goto('/?cef=1');
    await page.waitForSelector('canvas', { timeout: 10_000 });

    // Smoke: dispatch 10 wheel events with ~100ms spacing and measure round-trip
    // wall-clock time per iteration. PoC does not yet wire wheel→cef_host;
    // this only proves the WS+worker+renderer pipeline does not lock up.
    const latencies = await page.evaluate(async () => {
      const results: number[] = [];
      const canvas = document.querySelector('canvas');
      if (!canvas) return results;
      for (let i = 0; i < 10; i++) {
        const t0 = performance.now();
        canvas.dispatchEvent(
          new WheelEvent('wheel', { deltaY: 100, bubbles: true, cancelable: true }),
        );
        await new Promise((r) => setTimeout(r, 100));
        results.push(performance.now() - t0);
      }
      return results;
    });

    expect(latencies.length).toBe(10);
    latencies.sort((a, b) => a - b);
    const p95 = latencies[Math.floor(latencies.length * 0.95)] ?? 0;
    console.log(`PoC KPI p95: ${p95.toFixed(1)} ms (smoke; spec target ≤50 ms with real input)`);
    expect(p95).toBeLessThan(500);
  });

  test('REST: spawn a cef Browser activity via the daemon', async ({ request }) => {
    const sessionsRes = await request.get(`${DAEMON}/sessions`);
    expect(sessionsRes.status()).toBe(200);
    const sessions = (await sessionsRes.json()) as {
      sessions: { windows: string[] }[];
    };
    const wid = sessions.sessions[0]?.windows[0];
    expect(wid).toBeTruthy();
    if (!wid) return;

    const windowRes = await request.get(`${DAEMON}/windows/${wid}`);
    expect(windowRes.status()).toBe(200);
    const win = (await windowRes.json()) as { panes: { id: string }[] };
    const pid = win.panes[0]?.id;
    expect(pid).toBeTruthy();
    if (!pid) return;

    const post = await request.post(`${DAEMON}/windows/${wid}/panes/${pid}/activities`, {
      data: {
        activity: {
          activity_id: crypto.randomUUID(),
          kind: { type: 'browser', initial_url: 'https://example.com/' },
        },
      },
    });
    expect([200, 201]).toContain(post.status());
  });

  test('Phase A: a Browser activity paints at least one frame', async ({ page, request }) => {
    await page.goto('/?cef=1');
    await page.waitForSelector('canvas', { timeout: 10_000 });

    const sessionsRes = await request.get(`${DAEMON}/sessions`);
    expect(sessionsRes.status()).toBe(200);
    const sessions = (await sessionsRes.json()) as {
      sessions: { windows: string[] }[];
    };
    const wid = sessions.sessions[0]?.windows[0];
    expect(wid).toBeTruthy();
    if (!wid) return;

    const windowRes = await request.get(`${DAEMON}/windows/${wid}`);
    expect(windowRes.status()).toBe(200);
    const win = (await windowRes.json()) as { panes: { id: string }[] };
    const pid = win.panes[0]?.id;
    expect(pid).toBeTruthy();
    if (!pid) return;

    // Reset the paint counter before issuing the POST so we measure paints
    // from this activity, not residual ones from earlier tests in the file.
    await page.evaluate(() => {
      (window as unknown as { __poc_paint_done_count?: number }).__poc_paint_done_count = 0;
    });

    const post = await request.post(`${DAEMON}/windows/${wid}/panes/${pid}/activities`, {
      data: {
        activity: {
          activity_id: crypto.randomUUID(),
          kind: { type: 'browser', initial_url: 'https://example.com/' },
        },
      },
    });
    expect([200, 201]).toContain(post.status());

    // BrowserActivityCef wires `paint-done` into `window.__poc_paint_done_count`.
    // We can't read getImageData on a transferred OffscreenCanvas; this counter
    // is the closest proxy for "at least one keyframe rendered".
    await page.waitForFunction(
      () =>
        ((window as unknown as { __poc_paint_done_count?: number }).__poc_paint_done_count ?? 0) >
        0,
      { timeout: 30_000 },
    );
  });
});
