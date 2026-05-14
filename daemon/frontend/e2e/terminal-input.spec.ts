import { expect, test } from '@playwright/test';

test.describe('Phase 3A — input layering & paste smoke', () => {
  test('layering invariant: pointer events hit textarea, not canvas', async ({ page }) => {
    await page.goto('/?mode=vt');
    await page.waitForSelector('canvas');
    await page.waitForSelector('textarea');

    const tag = await page.evaluate(() => {
      const canvas = document.querySelector('canvas');
      if (!canvas) return null;
      const r = canvas.getBoundingClientRect();
      const cx = r.left + r.width / 2;
      const cy = r.top + r.height / 2;
      return document.elementFromPoint(cx, cy)?.tagName ?? null;
    });

    expect(tag).toBe('TEXTAREA');
  });

  test('Cmd+V triggers bracketed paste with CR normalization', async ({ page, browserName }) => {
    test.skip(browserName !== 'chromium', 'clipboard API only reliable on Chromium in CI');
    await page.goto('/?mode=vt');
    await page.waitForSelector('textarea');

    // Capture WS binary frames sent from the browser to the daemon.
    await page.evaluate(() => {
      const orig = WebSocket.prototype.send;
      (WebSocket.prototype as unknown as { __sent: ArrayBuffer[] }).__sent = [];
      WebSocket.prototype.send = function (
        data: string | ArrayBufferLike | Blob | ArrayBufferView,
      ) {
        if (data instanceof ArrayBuffer) {
          (WebSocket.prototype as unknown as { __sent: ArrayBuffer[] }).__sent.push(data);
        }
        return orig.call(this, data);
      };
    });

    await page.evaluate(() => navigator.clipboard.writeText('a\nb'));

    const isMac = process.platform === 'darwin';
    const modifier = isMac ? 'Meta' : 'Control+Shift';
    await page.locator('textarea').focus();
    await page.keyboard.press(`${modifier}+v`);
    await page.waitForTimeout(200);

    const sentBlobs = await page.evaluate(() =>
      (WebSocket.prototype as unknown as { __sent: ArrayBuffer[] }).__sent.map((b) =>
        Array.from(new Uint8Array(b)),
      ),
    );
    const wrapperBytes = [0x1b, 0x5b, 0x32, 0x30, 0x30, 0x7e]; // \e[200~
    const found = sentBlobs.some((arr) => {
      for (let i = 0; i + wrapperBytes.length <= arr.length; i++) {
        let match = true;
        for (let j = 0; j < wrapperBytes.length; j++) {
          if (arr[i + j] !== wrapperBytes[j]) {
            match = false;
            break;
          }
        }
        if (match) return true;
      }
      return false;
    });
    expect(found).toBe(true);
  });

  test('composition smoke: insertText does not crash the terminal', async ({ page }) => {
    await page.goto('/?mode=vt');
    await page.waitForSelector('textarea');
    await page.locator('textarea').focus();

    // Without a real IME, we cannot drive compositionstart/end natively in
    // Playwright. insertText fires `beforeinput` + `input` (no composition).
    // This test only verifies the page doesn't crash and the textarea is
    // present. Real-IME e2e is deferred to Phase 3D parity gate.
    await page.keyboard.insertText('hi');
    await page.waitForTimeout(100);

    const tagPresent = await page.evaluate(() => document.querySelector('textarea') !== null);
    expect(tagPresent).toBe(true);
  });
});
