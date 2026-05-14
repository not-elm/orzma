import { expect, test } from '@playwright/test';

test.describe('Phase 3B — visual overlays smoke', () => {
  test('cursor overlay mounts on the VT terminal', async ({ page }) => {
    await page.goto('/?mode=vt');
    await page.waitForSelector('canvas');
    // The DOM cursor lives as an absolutely-positioned div with data-testid.
    await expect(page.locator('[data-testid="vt-cursor"]')).toBeVisible({ timeout: 5000 });
  });

  test('plain pointer drag (no mouse mode) renders a selection rect', async ({ page }) => {
    await page.goto('/?mode=vt');
    await page.waitForSelector('canvas');

    const canvas = page.locator('canvas').first();
    const box = await canvas.boundingBox();
    if (!box) throw new Error('canvas has no bounding box');

    await page.mouse.move(box.x + 16, box.y + 16);
    await page.mouse.down();
    await page.mouse.move(box.x + 96, box.y + 32);
    await page.waitForTimeout(50);
    await page.mouse.up();

    // Selection overlay renders 1-3 [data-rect] divs.
    await expect(page.locator('[data-rect]').first()).toBeVisible({ timeout: 2000 });
  });

  test('hover over a plain URL underlines via the WebLinks fallback', async ({ page }) => {
    await page.goto('/?mode=vt');
    await page.waitForSelector('canvas');
    await page.locator('textarea').focus();

    // Inject a URL into the shell. The default shell echoes the command back
    // so the rendered row contains `https://example.com`.
    await page.keyboard.type('echo "see https://example.com here"');
    await page.keyboard.press('Enter');
    await page.waitForTimeout(500);

    // Read the actual canvas cell metrics — they vary by font / DPR.
    const metrics = await page.evaluate(() => {
      const canvas = document.querySelector('canvas');
      if (!canvas) return null;
      const r = canvas.getBoundingClientRect();
      return {
        left: r.left,
        top: r.top,
        cellW: parseFloat(canvas.style.width || '0') / 80,
        cellH: parseFloat(canvas.style.height || '0') / 24,
      };
    });
    if (!metrics) throw new Error('canvas has no metrics');

    // Walk row 0 hovering at one cell at a time. waitForTimeout between moves
    // gives the RAF-coalesced pointer listener a chance to flush.
    for (let col = 0; col < 80; col++) {
      const x = metrics.left + (col + 0.5) * metrics.cellW;
      const y = metrics.top + 0.5 * metrics.cellH;
      await page.mouse.move(x, y);
      await page.waitForTimeout(20);
      const n = await page.locator('[data-uri]').count();
      if (n > 0) break;
    }
    await expect(page.locator('[data-uri]')).toBeVisible({ timeout: 1000 });
  });
});
