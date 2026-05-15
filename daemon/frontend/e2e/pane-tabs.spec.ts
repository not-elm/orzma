import { expect, test } from '@playwright/test';

test.describe('pane activity tab bar', () => {
  test('Ctrl+B c adds a second tab and clicking it switches activity', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');

    // Bootstrap pane starts with exactly one tab.
    await expect(page.getByRole('tab')).toHaveCount(1);

    // Ctrl+B c spawns a second terminal activity.
    await page.keyboard.press('Control+b');
    await page.keyboard.press('c');
    await expect(page.getByRole('tab')).toHaveCount(2);

    // The newly-spawned activity is active; click the first (inactive) tab.
    const tabs = page.getByRole('tab');
    const firstTab = tabs.first();
    await expect(firstTab).toHaveAttribute('aria-selected', 'false');
    await firstTab.click();
    await expect(firstTab).toHaveAttribute('aria-selected', 'true');
  });
});
