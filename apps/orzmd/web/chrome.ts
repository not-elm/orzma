/** Severity of a toast message. */
export type ToastKind = 'error' | 'info';

/** Stage of the in-page search. */
export type SearchStage = 'closed' | 'typing' | 'active';

/** The chrome state the controller pushes with the `chrome` event. */
export interface Chrome {
  /** File name of the viewed document. */
  fileName: string;
  /** Whether the viewed file has been deleted. */
  missing: boolean;
  /** First key of a pending two-key chord, or `null`. */
  pendingKey: string | null;
  /** The message to show, or `null`. */
  toast: { kind: ToastKind; text: string } | null;
  /** Outline panel state; `selected` indexes the `id="h{n}"` headings. */
  outline: { open: boolean; selected: number };
  /** Stage of the in-page search. */
  search: SearchStage;
}

/** A document heading: its level (1-6) and its text. */
export interface HeadingInfo {
  level: number;
  text: string;
}

/** The text shown for a heading; a heading without text reads `(untitled)`. */
export function headingLabel(text: string): string {
  const trimmed = text.trim();
  return trimmed.length === 0 ? '(untitled)' : trimmed;
}

/**
 * Returns the labels of heading `current` and of each ancestor (the nearest earlier heading of a
 * shallower level, repeatedly), shallowest first. Empty when `current` is `null` or out of range.
 */
export function breadcrumb(headings: readonly HeadingInfo[], current: number | null): string[] {
  if (current === null || current < 0 || current >= headings.length) {
    return [];
  }
  const trail = [headings[current]];
  let level = headings[current].level;
  for (let i = current - 1; i >= 0; i--) {
    if (headings[i].level < level) {
      trail.push(headings[i]);
      level = headings[i].level;
    }
  }
  return trail.reverse().map((h) => headingLabel(h.text));
}

/**
 * Draws the rail: the file name, the crumbs (each behind a `›`), the pending chord key and the
 * deleted-file badge. Crumbs that do not fit are dropped from the shallow end behind a `…`.
 */
export function renderRail(rail: HTMLElement, chrome: Chrome, crumbs: readonly string[]): void {
  const file = rail.querySelector<HTMLElement>('.rail-file');
  const crumbBox = rail.querySelector<HTMLElement>('.rail-crumbs');
  const key = rail.querySelector<HTMLElement>('.rail-key');
  const missing = rail.querySelector<HTMLElement>('.rail-missing');
  if (file === null || crumbBox === null || key === null || missing === null) {
    return;
  }
  file.textContent = chrome.fileName;
  fitCrumbs(crumbBox, crumbs);
  key.hidden = chrome.pendingKey === null;
  key.textContent = chrome.pendingKey ?? '';
  missing.hidden = !chrome.missing;
}

/** Shows `toast` in `el`, or hides `el` when `toast` is `null`. */
export function renderToast(el: HTMLElement, toast: Chrome['toast']): void {
  el.hidden = toast === null;
  el.classList.toggle('toast-error', toast?.kind === 'error');
  el.classList.toggle('toast-info', toast?.kind === 'info');
  const text = el.querySelector<HTMLElement>('.toast-text');
  if (text !== null) {
    text.textContent = toast?.text ?? '';
  }
}

function fitCrumbs(box: HTMLElement, crumbs: readonly string[]): void {
  let shown = [...crumbs];
  fillCrumbs(box, shown, false);
  while (shown.length > 1 && box.scrollWidth > box.clientWidth) {
    shown = shown.slice(1);
    fillCrumbs(box, shown, true);
  }
}

function fillCrumbs(box: HTMLElement, crumbs: readonly string[], elided: boolean): void {
  const parts = elided ? ['…', ...crumbs] : crumbs;
  box.replaceChildren(
    ...parts.flatMap((text) => {
      const sep = document.createElement('span');
      sep.className = 'rail-sep';
      sep.textContent = '›';
      const part = document.createElement('span');
      part.textContent = text;
      return [sep, part];
    }),
  );
}
