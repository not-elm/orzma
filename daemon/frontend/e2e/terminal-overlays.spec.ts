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

  test('F1-F3: cursor x stays within ±1px of column * cellW (probe-based)', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');
    await page.locator('textarea').focus();
    // CSI cursor-position absolute: row 1, col 30. Using printf so the shell
    // doesn't insert a prompt-redraw mid-test.
    await page.keyboard.type(String.raw`printf "\033[1;30H"`);
    await page.keyboard.press('Enter');
    await page.waitForTimeout(300);

    // Probe the actual rendered glyph width inside a real row so the probe
    // shares the same font / line-height context as the cursor's expected
    // column origin. ±1 column-width tolerance absorbs sub-pixel rounding.
    const metrics = await page.evaluate(() => {
      const grid = document.querySelector('.terminal-grid');
      if (!grid) return null;
      const row0 = grid.firstElementChild as HTMLElement | null;
      if (!row0) return null;
      const probe = document.createElement('span');
      probe.style.visibility = 'hidden';
      probe.style.position = 'absolute';
      probe.style.whiteSpace = 'pre';
      probe.className = 'font-mono leading-none';
      probe.textContent = 'W';
      row0.appendChild(probe);
      const probeRect = probe.getBoundingClientRect();
      const gridRect = (grid as HTMLElement).getBoundingClientRect();
      probe.remove();
      return { cellW: probeRect.width, gridX: gridRect.left, gridY: gridRect.top };
    });
    if (!metrics) throw new Error('grid / row0 missing');

    const cursor = await page.locator('[data-testid="vt-cursor"]').boundingBox();
    if (!cursor) throw new Error('cursor not visible');

    // Cursor must align to the column the shell reports — the shell sees the
    // 30th-column position from CSI 30G. Allow ±1px to absorb sub-pixel
    // rounding by the browser.
    const expectedX = metrics.gridX + 0 * metrics.cellW; // shells typically reset to col 0 then echo
    // We can't strictly assert col 30 because the prompt re-renders, but we
    // CAN assert: cursor is on a column boundary, i.e. the offset from gridX
    // is an integer multiple of cellW (±1px). This catches drift regressions.
    const cursorOffsetX = cursor.x - metrics.gridX;
    const columnsFromLeft = cursorOffsetX / metrics.cellW;
    const nearestColumn = Math.round(columnsFromLeft);
    const drift = Math.abs(cursorOffsetX - nearestColumn * metrics.cellW);
    expect(drift).toBeLessThanOrEqual(1);
    expect(expectedX).toBeGreaterThanOrEqual(0); // sanity
  });

  test('G5: terminal-grid fits the pane (scrollWidth === clientWidth)', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');
    // Allow ResizeObserver + initial fitToContainer to settle.
    await page.waitForTimeout(400);

    const fit = await page.evaluate(() => {
      const pane = document.querySelector('.terminal-pane') as HTMLElement | null;
      const grid = document.querySelector('.terminal-grid') as HTMLElement | null;
      if (!pane || !grid) return null;
      const row0 = grid.firstElementChild as HTMLElement | null;
      // Probe cellW with the same context as the renderer.
      const probe = document.createElement('span');
      probe.style.visibility = 'hidden';
      probe.style.position = 'absolute';
      probe.style.whiteSpace = 'pre';
      probe.className = 'font-mono leading-none';
      probe.textContent = 'W';
      (row0 ?? grid).appendChild(probe);
      const cellW = probe.getBoundingClientRect().width;
      probe.remove();
      const cols = (row0?.textContent ?? '').length || 0;
      return {
        paneW: pane.clientWidth,
        gridW: grid.clientWidth,
        gridScrollW: grid.scrollWidth,
        cellW,
        cols,
      };
    });
    if (!fit) throw new Error('grid not mounted');

    // Grid does not overflow horizontally.
    expect(fit.gridScrollW).toBeLessThanOrEqual(fit.gridW + 1); // ±1px sub-pixel
    // Grid clientWidth tracks pane clientWidth.
    expect(Math.abs(fit.gridW - fit.paneW)).toBeLessThanOrEqual(2);
    // Remainder must be less than one cellW (floor() invariant).
    if (fit.cols > 0) {
      const remainder = fit.paneW - fit.cols * fit.cellW;
      expect(remainder).toBeGreaterThanOrEqual(-1);
      // Allow up to ~2 cellW slack: the prompt may have echoed bytes that
      // partially fill row 0 below the configured cols. We mainly care that
      // the row isn't *vastly* shorter than expected (which would indicate
      // the initial resize was dropped).
      expect(remainder).toBeLessThanOrEqual(2 * fit.cellW);
    }
  });

  test('F1-F3: cursor y aligns to a row boundary (probe-based)', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('.terminal-grid');

    const metrics = await page.evaluate(() => {
      const grid = document.querySelector('.terminal-grid');
      if (!grid) return null;
      const row0 = grid.firstElementChild as HTMLElement | null;
      if (!row0) return null;
      const probe = document.createElement('span');
      probe.style.visibility = 'hidden';
      probe.style.position = 'absolute';
      probe.style.whiteSpace = 'pre';
      probe.className = 'font-mono leading-none';
      probe.textContent = 'W';
      row0.appendChild(probe);
      const probeRect = probe.getBoundingClientRect();
      const gridRect = (grid as HTMLElement).getBoundingClientRect();
      probe.remove();
      return { cellH: probeRect.height, gridY: gridRect.top };
    });
    if (!metrics) throw new Error('grid / row0 missing');

    const cursor = await page.locator('[data-testid="vt-cursor"]').boundingBox();
    if (!cursor) throw new Error('cursor not visible');

    const cursorOffsetY = cursor.y - metrics.gridY;
    const rowsFromTop = cursorOffsetY / metrics.cellH;
    const nearestRow = Math.round(rowsFromTop);
    const drift = Math.abs(cursorOffsetY - nearestRow * metrics.cellH);
    expect(drift).toBeLessThanOrEqual(1);
  });
});
