import { expect, type Page, test } from '@playwright/test';

interface CellProbe {
  cellW: number;
  cellH: number;
  gridX: number;
  gridY: number;
}

/** Measures the rendered monospace cell inside .terminal-grid's first row.
 *  Mirrors `renderer/font.ts::cellWidthOf` / `cellHeightOf` so the e2e
 *  assertions test against the same font environment the renderer uses. */
async function probeCell(page: Page): Promise<CellProbe> {
  const result = await page.evaluate(() => {
    const grid = document.querySelector('.terminal-grid') as HTMLElement | null;
    if (!grid) return null;
    const row0 = grid.firstElementChild as HTMLElement | null;
    const host = row0 ?? grid;
    const probe = document.createElement('span');
    probe.style.visibility = 'hidden';
    probe.style.position = 'absolute';
    probe.style.whiteSpace = 'pre';
    probe.className = 'font-mono leading-none';
    probe.textContent = 'W';
    host.appendChild(probe);
    const probeRect = probe.getBoundingClientRect();
    const gridRect = grid.getBoundingClientRect();
    probe.remove();
    return {
      cellW: probeRect.width,
      cellH: probeRect.height,
      gridX: gridRect.left,
      gridY: gridRect.top,
    };
  });
  if (!result) throw new Error('probeCell: .terminal-grid not mounted');
  return result;
}

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

  test('cursor x stays within ±1px of an integer column boundary', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');
    await page.locator('textarea').focus();
    await page.keyboard.type(String.raw`printf "\033[1;30H"`);
    await page.keyboard.press('Enter');
    await page.waitForTimeout(300);

    const { cellW, gridX } = await probeCell(page);
    const cursor = await page.locator('[data-testid="vt-cursor"]').boundingBox();
    if (!cursor) throw new Error('cursor not visible');

    const cursorOffsetX = cursor.x - gridX;
    const nearestColumn = Math.round(cursorOffsetX / cellW);
    const drift = Math.abs(cursorOffsetX - nearestColumn * cellW);
    expect(drift).toBeLessThanOrEqual(1);
  });

  test('terminal-grid fits the pane (no horizontal overflow, tracks pane width)', async ({
    page,
  }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');
    // ResizeObserver fires async; give it a frame to settle.
    await page.waitForTimeout(400);

    const fit = await page.evaluate(() => {
      const pane = document.querySelector('.terminal-pane') as HTMLElement | null;
      const grid = document.querySelector('.terminal-grid') as HTMLElement | null;
      if (!pane || !grid) return null;
      return {
        paneW: pane.clientWidth,
        paneH: pane.clientHeight,
        gridW: grid.clientWidth,
        gridScrollW: grid.scrollWidth,
        gridH: grid.clientHeight,
        gridScrollH: grid.scrollHeight,
        rows: grid.children.length,
      };
    });
    if (!fit) throw new Error('grid not mounted');
    const { cellW, cellH } = await probeCell(page);

    // The two invariants that actually catch the "not fitted" symptom:
    //  1. grid width matches the pane (within sub-pixel rounding).
    //  2. grid contents do not overflow the grid box.
    // If G1's WebSocket OPEN race regressed, the server would stay at the
    // spawn-default 80×24, and on a wider pane gridW would be much less
    // than paneW. On a narrower pane gridScrollW would exceed gridW.
    expect(fit.gridScrollW).toBeLessThanOrEqual(fit.gridW + 1);
    expect(fit.gridScrollH).toBeLessThanOrEqual(fit.gridH + 1);
    // Pane width gets fully consumed except for the floor() remainder
    // (< cellW). If the resize was dropped, the gap would be many cells.
    expect(fit.paneW - fit.gridW).toBeLessThan(cellW);
    // Number of rows matches floor(paneH / cellH) within ±1.
    const expectedRows = Math.floor(fit.paneH / cellH);
    expect(Math.abs(fit.rows - expectedRows)).toBeLessThanOrEqual(1);
  });

  test('cursor y aligns to an integer row boundary', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');

    const { cellH, gridY } = await probeCell(page);
    const cursor = await page.locator('[data-testid="vt-cursor"]').boundingBox();
    if (!cursor) throw new Error('cursor not visible');

    const cursorOffsetY = cursor.y - gridY;
    const nearestRow = Math.round(cursorOffsetY / cellH);
    const drift = Math.abs(cursorOffsetY - nearestRow * cellH);
    expect(drift).toBeLessThanOrEqual(1);
  });
});
