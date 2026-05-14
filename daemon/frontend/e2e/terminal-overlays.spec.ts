import { expect, test } from '@playwright/test';

test.describe('Phase 3.5 — DOM renderer smoke', () => {
  test('terminal-grid mounts under VtTerminal', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');
    await expect(page.locator('.terminal-grid')).toBeVisible({ timeout: 5000 });
  });

  test('cursor overlay mounts', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');
    await expect(page.locator('[data-testid="vt-cursor"]')).toBeVisible({ timeout: 5000 });
  });

  test('terminal-grid text content is selectable via document.getSelection()', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');
    await page.locator('textarea').focus();
    await page.keyboard.type('hello world');
    await page.waitForTimeout(500);

    // Programmatically select the first row using Range API. If user-select:text
    // is in effect on the grid, getSelection().toString() returns the text.
    const selected = await page.evaluate(() => {
      const grid = document.querySelector('.terminal-grid');
      if (!grid) return null;
      const range = document.createRange();
      range.selectNodeContents(grid);
      const sel = document.getSelection();
      if (!sel) return null;
      sel.removeAllRanges();
      sel.addRange(range);
      return sel.toString();
    });
    expect(selected).not.toBeNull();
    expect((selected ?? '').length).toBeGreaterThan(0);
    // We can't assert "hello" specifically — the shell prompt may rewrite
    // the line. Just verify the grid contains user-select:text content.
  });

  test('hover over a plain URL renders <a href> via WebLinks regex', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');
    await page.locator('textarea').focus();
    await page.keyboard.type('echo "see https://example.com here"');
    await page.keyboard.press('Enter');
    await page.waitForTimeout(500);

    await expect(page.locator('a[href*="example.com"]').first()).toBeVisible({ timeout: 2000 });
  });

  test('R7: javascript: URI does not produce an <a href>', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');
    await page.locator('textarea').focus();
    await page.keyboard.type('echo "click javascript:alert(1) here"');
    await page.keyboard.press('Enter');
    await page.waitForTimeout(500);

    const count = await page.locator('a[href^="javascript:"]').count();
    expect(count).toBe(0);
  });
});
