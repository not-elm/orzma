import { decode, schemeOf } from './url';

/**
 * Whether `raw` (an `<img src>` attribute value) is a local filesystem path
 * ozmd should stage. Excludes in-page anchors, protocol-relative URLs, and any
 * URL carrying a scheme (`http:`, `https:`, `data:`, `blob:`, `file:`, …).
 */
export function isLocalImageSrc(raw: string): boolean {
  const s = raw.trim();
  if (s.length === 0 || s.startsWith('#') || s.startsWith('//')) {
    return false;
  }
  return schemeOf(s) === null;
}

/** Strips any `?query` / `#fragment` from `raw` and percent-decodes the rest. */
export function toLocalPath(raw: string): string {
  const s = raw.trim();
  const noFragment = s.split('#', 1)[0];
  const noQuery = noFragment.split('?', 1)[0];
  return decode(noQuery);
}

/**
 * Finds every `<img>` under `root` with a local `src` (read via
 * `getAttribute('src')`, i.e. the raw authored path), paired with its
 * decoded filesystem path.
 */
export function collectLocalImages(root: HTMLElement): { el: HTMLImageElement; path: string }[] {
  const out: { el: HTMLImageElement; path: string }[] = [];
  for (const el of root.querySelectorAll('img')) {
    const raw = el.getAttribute('src');
    if (raw !== null && isLocalImageSrc(raw)) {
      out.push({ el, path: toLocalPath(raw) });
    }
  }
  return out;
}
