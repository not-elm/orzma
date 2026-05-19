/**
 * Resolves toolbar input into a URL to navigate to. Mirrors Chrome's omnibox /
 * Firefox's URL Bar Algorithm: input that looks like a URL is treated as one,
 * everything else is sent to a search engine via the configured template.
 */

/** Template placeholder substituted with `encodeURIComponent(query)`. */
const QUERY_PLACEHOLDER = '{query}';

/** Default DuckDuckGo template used when no config override is supplied. */
export const DEFAULT_SEARCH_TEMPLATE = `https://duckduckgo.com/?q=${QUERY_PLACEHOLDER}`;

const SCHEME_RE = /^[a-z][a-z0-9+\-.]*:\/\//i;
const KNOWN_SCHEME_PREFIXES = ['about:', 'data:', 'chrome:', 'file:', 'view-source:'];
const IPV4_RE = /^(\d{1,3}\.){3}\d{1,3}$/;
const HOSTNAME_LABEL = '[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?';
const DOMAIN_RE = new RegExp(`^${HOSTNAME_LABEL}(\\.${HOSTNAME_LABEL})*$`);

/**
 * Decides whether `input` should be navigated to as a URL or searched.
 * Returns the resolved URL string ready for `Navigate`.
 *
 * Rules (mirroring Firefox / Min browser):
 *  1. Empty → empty (caller skips navigate).
 *  2. Starts with a scheme (`https://`, `about:`, `data:` …) → as-is.
 *  3. Starts with `?` → search the rest.
 *  4. Whitespace or `"` before the first `.` / `:` / `?` → search.
 *  5. First path segment looks like a host (IPv4, `localhost`, `host:port`,
 *     or a dotted domain with a 2+ char TLD) → prepend `https://`.
 *  6. Otherwise → search.
 */
export function resolveOmniboxInput(
  rawInput: string,
  searchTemplate: string = DEFAULT_SEARCH_TEMPLATE,
): string {
  const input = rawInput.trim();
  if (input.length === 0) return '';

  if (SCHEME_RE.test(input)) return input;
  if (KNOWN_SCHEME_PREFIXES.some((p) => input.startsWith(p))) return input;

  if (input.startsWith('?')) return searchFor(input.slice(1).trimStart(), searchTemplate);

  const firstStructural = input.search(/[.:?]/);
  const firstSpace = input.search(/[\s"]/);
  if (firstSpace !== -1 && (firstStructural === -1 || firstSpace < firstStructural)) {
    return searchFor(input, searchTemplate);
  }

  const hostPart = input.split(/[/?#]/, 1)[0];
  if (looksLikeHost(hostPart)) return `https://${input}`;

  return searchFor(input, searchTemplate);
}

function searchFor(query: string, template: string): string {
  return template.replace(QUERY_PLACEHOLDER, encodeURIComponent(query));
}

function looksLikeHost(host: string): boolean {
  if (host.length === 0) return false;

  const [hostname, ...rest] = host.split(':');
  if (rest.length > 1) return false;
  if (rest.length === 1 && !/^\d+$/.test(rest[0])) return false;

  if (hostname === 'localhost') return true;
  if (IPV4_RE.test(hostname)) return true;
  if (!hostname.includes('.')) return false;
  if (!DOMAIN_RE.test(hostname)) return false;

  const labels = hostname.split('.');
  const tld = labels[labels.length - 1];
  return tld.length >= 2 && /^[A-Za-z]+$/.test(tld);
}
