/**
 * Browser-activity configuration fetched from `GET /configs/browser`. Held in
 * a module singleton so `BrowserActivity` can read it synchronously when
 * rendering the toolbar. `loadBrowserConfig` is awaited before the first
 * render — see `main.tsx`.
 */
import { DEFAULT_SEARCH_TEMPLATE } from '../browser/omnibox';
import { fetchJson } from '../fetchJson';

/** Resolved browser configuration (camelCase view of the API JSON). */
export interface BrowserConfig {
  searchTemplate: string;
}

const DEFAULT_BROWSER_CONFIG: BrowserConfig = {
  searchTemplate: DEFAULT_SEARCH_TEMPLATE,
};

let current: BrowserConfig = DEFAULT_BROWSER_CONFIG;

/** Returns the active browser configuration. */
export function getBrowserConfig(): BrowserConfig {
  return current;
}

/** Fetches `/configs/browser` and updates the singleton. On any failure the
 *  singleton is left at (or reset to) the built-in defaults. */
export async function loadBrowserConfig(): Promise<void> {
  try {
    const raw = (await fetchJson('/configs/browser')) as Record<string, unknown>;
    current = {
      searchTemplate:
        typeof raw.search_template === 'string' && raw.search_template.length > 0
          ? raw.search_template
          : DEFAULT_BROWSER_CONFIG.searchTemplate,
    };
  } catch (e) {
    console.warn('loadBrowserConfig: failed to load or parse browser config, using defaults', e);
    current = DEFAULT_BROWSER_CONFIG;
  }
}
