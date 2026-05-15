import { expect, test } from '@playwright/test';

// Gate: launching a real Browser Activity spawns a Chromium child process inside
// the daemon. Opt-in via OZMUX_TEST_REAL_CHROME=1 so CI without a local Chrome
// installation does not fail.
const HAS_CHROME = process.platform === 'darwin' && process.env.OZMUX_TEST_REAL_CHROME === '1';

const DAEMON = 'http://localhost:3200';

test.describe('Browser Activity e2e', () => {
  test.skip(!HAS_CHROME, 'OZMUX_TEST_REAL_CHROME=1 required (launches real Chromium)');

  test('opens a browser activity and renders a JPEG screencast canvas', async ({
    page,
    request,
  }) => {
    await page.goto('/');
    // Wait for the default terminal activity to appear so the daemon is fully
    // bootstrapped before we issue REST calls.
    await page.waitForSelector('.terminal-grid');

    // Step 1: discover the active window id via the sessions REST tree.
    const sessionsResp = await request.get(`${DAEMON}/sessions`);
    expect(sessionsResp.ok(), 'GET /sessions').toBeTruthy();
    const sessionsBody = await sessionsResp.json();
    const sid: string = sessionsBody.sessions[0]?.id;
    expect(sid, 'at least one session').toBeDefined();

    const sessionResp = await request.get(`${DAEMON}/sessions/${sid}`);
    expect(sessionResp.ok(), 'GET /sessions/:sid').toBeTruthy();
    const sessionBody = await sessionResp.json();
    const wid: string = sessionBody.active_window ?? sessionBody.linkedWindows?.[0];
    expect(wid, 'active window').toBeDefined();

    // Step 2: discover the first pane id from the window view.
    const windowResp = await request.get(`${DAEMON}/windows/${wid}`);
    expect(windowResp.ok(), 'GET /windows/:wid').toBeTruthy();
    const windowBody = await windowResp.json();
    const pid: string = windowBody.panes[0]?.id;
    expect(pid, 'at least one pane').toBeDefined();

    // Step 3: POST a browser activity.
    const aid = crypto.randomUUID();
    const createResp = await request.post(`${DAEMON}/windows/${wid}/panes/${pid}/activities`, {
      data: {
        activity: {
          activity_id: aid,
          kind: { type: 'browser', initial_url: 'https://example.com' },
        },
      },
    });
    expect(createResp.ok(), 'POST activity').toBeTruthy();

    // Step 4: activate it so the frontend renders BrowserActivity.
    const activateResp = await request.post(
      `${DAEMON}/windows/${wid}/panes/${pid}/activities/${aid}/activate`,
    );
    expect(activateResp.ok(), 'activate').toBeTruthy();

    // Step 5: wait for the toolbar to appear — proves the frontend switched to
    // the BrowserActivity component.
    await page.waitForSelector('[aria-label="Browser viewport"]', { timeout: 10_000 });

    // The toolbar buttons are rendered by Toolbar.tsx with these aria-labels.
    await expect(page.getByRole('button', { name: 'Back' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Forward' })).toBeVisible();

    // Step 6: wait for a non-zero canvas backing size. The CanvasFrame component
    // sets canvas.width / canvas.height only once the first screencast frame
    // arrives from Chromium, so polling here validates the entire pipeline:
    // daemon BrowserService → WebSocket → useBrowserSocket → CanvasFrame paint.
    await expect
      .poll(
        async () => {
          return page
            .locator('canvas')
            .first()
            .evaluate((el: HTMLCanvasElement) => el.width > 0 && el.height > 0)
            .catch(() => false);
        },
        { timeout: 15_000, message: 'canvas never received a screencast frame' },
      )
      .toBe(true);

    // Step 7: visual smoke — saved to test-results/ for human inspection on failure.
    await page.screenshot({ path: 'test-results/browser-activity.png', fullPage: false });
  });
});
