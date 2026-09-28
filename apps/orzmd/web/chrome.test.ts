import { describe, expect, it } from 'vitest';
import { breadcrumb, type Chrome, headingLabel, renderRail, renderToast } from './chrome';

const heads = [
  { level: 1, text: 'Configuration' },
  { level: 2, text: 'Settings' },
  { level: 3, text: '[orzma]' },
  { level: 3, text: '[inactive_pane]' },
  { level: 2, text: 'Validation' },
];

function chrome(overrides: Partial<Chrome> = {}): Chrome {
  return {
    fileName: 'configuration.md',
    missing: false,
    pendingKey: null,
    toast: null,
    outline: { open: false, selected: 0 },
    search: 'closed',
    ...overrides,
  };
}

function railElement(): HTMLElement {
  const rail = document.createElement('header');
  rail.innerHTML =
    '<span class="rail-file"></span><span class="rail-crumbs"></span>' +
    '<kbd class="rail-key" hidden></kbd><span class="rail-missing" hidden>File deleted</span>';
  return rail;
}

describe('breadcrumb', () => {
  it('lists the current heading and its ancestors, shallowest first', () => {
    expect(breadcrumb(heads, 3)).toEqual(['Configuration', 'Settings', '[inactive_pane]']);
  });

  it('returns nothing above the first heading', () => {
    expect(breadcrumb(heads, null)).toEqual([]);
    expect(breadcrumb(heads, 9)).toEqual([]);
  });

  it('follows skipped levels', () => {
    const skipped = [
      { level: 1, text: 'A' },
      { level: 3, text: 'C' },
    ];
    expect(breadcrumb(skipped, 1)).toEqual(['A', 'C']);
  });

  it('labels a heading without text', () => {
    expect(breadcrumb([{ level: 2, text: '  ' }], 0)).toEqual([headingLabel('')]);
    expect(headingLabel('')).toBe('(untitled)');
  });
});

describe('renderRail', () => {
  it('shows the file name and the crumbs behind separators', () => {
    const rail = railElement();
    renderRail(rail, chrome(), ['Settings', '[inactive_pane]']);
    expect(rail.querySelector('.rail-file')?.textContent).toBe('configuration.md');
    expect(rail.querySelector('.rail-crumbs')?.textContent).toBe('›Settings›[inactive_pane]');
  });

  it('shows the pending key and the deleted badge only when set', () => {
    const rail = railElement();
    renderRail(rail, chrome(), []);
    expect((rail.querySelector('.rail-key') as HTMLElement).hidden).toBe(true);
    expect((rail.querySelector('.rail-missing') as HTMLElement).hidden).toBe(true);
    renderRail(rail, chrome({ pendingKey: 'g', missing: true }), []);
    expect(rail.querySelector('.rail-key')?.textContent).toBe('g');
    expect((rail.querySelector('.rail-key') as HTMLElement).hidden).toBe(false);
    expect((rail.querySelector('.rail-missing') as HTMLElement).hidden).toBe(false);
  });
});

describe('renderToast', () => {
  it('shows an error toast and hides it again', () => {
    const el = document.createElement('div');
    el.innerHTML = '<span class="toast-icon"></span><span class="toast-text"></span>';
    renderToast(el, { kind: 'error', text: 'cannot open b.md' });
    expect(el.hidden).toBe(false);
    expect(el.classList.contains('toast-error')).toBe(true);
    expect(el.querySelector('.toast-text')?.textContent).toBe('cannot open b.md');
    renderToast(el, null);
    expect(el.hidden).toBe(true);
  });

  it('marks an info toast as info', () => {
    const el = document.createElement('div');
    el.innerHTML = '<span class="toast-icon"></span><span class="toast-text"></span>';
    renderToast(el, { kind: 'info', text: 'no previous page' });
    expect(el.classList.contains('toast-info')).toBe(true);
    expect(el.classList.contains('toast-error')).toBe(false);
  });
});
