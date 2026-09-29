import { describe, expect, it } from 'vitest';
import { breadcrumb, type Chrome, headingLabel, renderRail, ToastView } from './chrome';

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
    toast: null,
    ...overrides,
  };
}

function toastElement(): HTMLElement {
  const el = document.createElement('div');
  el.innerHTML = '<span class="toast-icon"></span><span class="toast-text"></span>';
  return el;
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
    renderRail(rail, chrome(), ['Settings', '[inactive_pane]'], null);
    expect(rail.querySelector('.rail-file')?.textContent).toBe('configuration.md');
    expect(rail.querySelector('.rail-crumbs')?.textContent).toBe('›Settings›[inactive_pane]');
  });

  it('shows the pending key and the deleted badge only when set', () => {
    const rail = railElement();
    renderRail(rail, chrome(), [], null);
    expect((rail.querySelector('.rail-key') as HTMLElement).hidden).toBe(true);
    expect((rail.querySelector('.rail-missing') as HTMLElement).hidden).toBe(true);
    renderRail(rail, chrome({ missing: true }), [], 'g');
    expect(rail.querySelector('.rail-key')?.textContent).toBe('g');
    expect((rail.querySelector('.rail-key') as HTMLElement).hidden).toBe(false);
    expect((rail.querySelector('.rail-missing') as HTMLElement).hidden).toBe(false);
  });
});

describe('ToastView', () => {
  const broken = { id: 1, kind: 'error' as const, text: 'cannot open b.md' };

  it('shows an error toast and hides it again', () => {
    const el = toastElement();
    const view = new ToastView(el);
    view.show(broken);
    expect(el.hidden).toBe(false);
    expect(el.classList.contains('toast-error')).toBe(true);
    expect(el.querySelector('.toast-text')?.textContent).toBe('cannot open b.md');
    view.show(null);
    expect(el.hidden).toBe(true);
  });

  it('marks an info toast as info', () => {
    const el = toastElement();
    new ToastView(el).show({ id: 1, kind: 'info', text: 'no previous page' });
    expect(el.classList.contains('toast-info')).toBe(true);
    expect(el.classList.contains('toast-error')).toBe(false);
  });

  it('keeps a dismissed toast hidden when the same toast is sent again', () => {
    const el = toastElement();
    const view = new ToastView(el);
    view.show(broken);
    view.dismiss();
    expect(el.hidden).toBe(true);
    view.show(broken);
    expect(el.hidden).toBe(true);
  });

  it('shows the next toast after one was dismissed', () => {
    const el = toastElement();
    const view = new ToastView(el);
    view.show(broken);
    view.dismiss();
    view.show({ id: 2, kind: 'error', text: 'cannot open c.md' });
    expect(el.hidden).toBe(false);
    expect(el.querySelector('.toast-text')?.textContent).toBe('cannot open c.md');
  });

  it('does nothing when dismissed with no toast on screen', () => {
    const el = toastElement();
    const view = new ToastView(el);
    view.dismiss();
    view.show(broken);
    expect(el.hidden).toBe(false);
  });
});
