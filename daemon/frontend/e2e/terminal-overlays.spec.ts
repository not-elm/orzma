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

    // Inject a URL into the shell. The default shell echoes back; we expect
    // the rendered row to contain `https://example.com`.
    await page.keyboard.type('echo "see https://example.com here"');
    await page.keyboard.press('Enter');
    await page.waitForTimeout(300);

    const canvas = page.locator('canvas').first();
    const box = await canvas.boundingBox();
    if (!box) throw new Error('canvas has no bounding box');
    // Hover roughly over the URL. Coordinates depend on font; just walk
    // along the row to maximize the chance of landing on the link cells.
    for (let dx = 30; dx < 280; dx += 8) {
      await page.mouse.move(box.x + dx, box.y + 8);
      const found = await page.locator('[data-uri]').count();
      if (found > 0) break;
    }
    await expect(page.locator('[data-uri]')).toBeVisible({ timeout: 2000 });
  });
});
