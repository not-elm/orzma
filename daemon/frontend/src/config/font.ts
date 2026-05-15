/**
 * Terminal font configuration fetched from `GET /configs/font`. Held in a
 * module singleton so the renderer's runtime stylesheet (`palette.ts`) can
 * read it synchronously. `loadFontConfig` must resolve before the first
 * render — see `main.tsx`.
 */
import { fetchJson } from '../fetchJson';

/** Resolved terminal font configuration (camelCase view of the API JSON). */
export interface FontConfig {
  size: number;
  normalFamily: string;
  boldFamily: string;
  italicFamily: string;
  boldItalicFamily: string;
}

const FALLBACK_FAMILY = 'monospace';

const DEFAULT_FONT_CONFIG: FontConfig = {
  size: 11.25,
  normalFamily: FALLBACK_FAMILY,
  boldFamily: FALLBACK_FAMILY,
  italicFamily: FALLBACK_FAMILY,
  boldItalicFamily: FALLBACK_FAMILY,
};

/** Converts a points value to CSS pixels (CSS defines 1pt = 4/3 px). */
export function pointsToPx(points: number): number {
  return (points * 4) / 3;
}

let current: FontConfig = DEFAULT_FONT_CONFIG;

/** Returns the active terminal font configuration. */
export function getFontConfig(): FontConfig {
  return current;
}

function str(value: unknown, fallback: string): string {
  return typeof value === 'string' && value.length > 0 ? value : fallback;
}

/** Fetches `/configs/font` and updates the singleton. On any failure the
 *  singleton is left at (or reset to) the built-in defaults. */
export async function loadFontConfig(): Promise<void> {
  try {
    const raw = (await fetchJson('/configs/font')) as Record<string, unknown>;
    current = {
      size: typeof raw.size === 'number' && raw.size > 0 ? raw.size : DEFAULT_FONT_CONFIG.size,
      normalFamily: str(raw.normal_family, FALLBACK_FAMILY),
      boldFamily: str(raw.bold_family, FALLBACK_FAMILY),
      italicFamily: str(raw.italic_family, FALLBACK_FAMILY),
      boldItalicFamily: str(raw.bold_italic_family, FALLBACK_FAMILY),
    };
  } catch (e) {
    console.warn('loadFontConfig: failed to load or parse font config, using defaults', e);
    current = DEFAULT_FONT_CONFIG;
  }
}

/** Forces each configured family into the browser FontFaceSet so cell
 *  metrics are probed against the real font. `document.fonts.ready` alone
 *  is unreliable for OS-installed (non-`@font-face`) fonts. */
export async function preloadFonts(): Promise<void> {
  if (typeof document === 'undefined' || !document.fonts) return;
  const families = new Set([
    current.normalFamily,
    current.boldFamily,
    current.italicFamily,
    current.boldItalicFamily,
  ]);
  await Promise.all(
    [...families].map(async (family) => {
      try {
        await document.fonts.load(`${pointsToPx(current.size)}px ${family}`);
      } catch {
        // Unparseable / unknown family — the CSS fallback handles it.
      }
    }),
  );
}
