import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { createGrid } from './grid';
import { createGridStore, GridStoreContext } from './grid-store';
import { TerminalGrid } from './TerminalGrid';

const fakeFm = { cellW: 8, cellH: 16, baseline: 12, fontCss: '14px monospace', dpr: 1 };
const noHyperlinks = new Map<number, string>();

function renderWithStore(g: ReturnType<typeof createGrid>) {
  const store = createGridStore();
  store.setGrid(g);
  return render(
    <GridStoreContext.Provider value={store}>
      <TerminalGrid fm={fakeFm} hyperlinks={noHyperlinks} />
    </GridStoreContext.Provider>,
  );
}

describe('TerminalGrid', () => {
  it('renders one row <div> per grid row', () => {
    const g = createGrid({ cols: 5, rows: 3 });
    g.cells[0] = [{ text: 'a', width: 1, fg: null, bg: null, style: 0 }];
    g.cells[1] = [{ text: 'b', width: 1, fg: null, bg: null, style: 0 }];
    g.cells[2] = [{ text: 'c', width: 1, fg: null, bg: null, style: 0 }];
    const { container } = renderWithStore(g);
    const rows = container.querySelectorAll('.block.whitespace-pre');
    expect(rows.length).toBe(3);
  });

  it('container has role="presentation" + aria-hidden="true" + text-foreground', () => {
    const g = createGrid({ cols: 5, rows: 1 });
    const { container } = renderWithStore(g);
    const grid = container.firstElementChild as HTMLElement;
    expect(grid.getAttribute('role')).toBe('presentation');
    expect(grid.getAttribute('aria-hidden')).toBe('true');
    expect(grid.className).toContain('text-foreground');
    expect(grid.className).toContain('terminal-grid');
  });
});
