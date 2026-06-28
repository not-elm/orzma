/** A classified `<a href>` target: where a click should be routed. */
export type LinkTarget =
  | { kind: 'anchor'; fragment: string }
  | { kind: 'markdown'; path: string; fragment: string | null }
  | { kind: 'file'; path: string }
  | { kind: 'external'; url: string }
  | { kind: 'ignore' };

const EXTERNAL_SCHEMES = new Set(['http', 'https', 'mailto', 'tel']);

/**
 * Classifies a raw `href` (the attribute string, not the browser-resolved URL)
 * into the action the click handler should take. Local paths are percent-decoded
 * so the controller receives real filesystem paths.
 */
export function classifyLink(href: string): LinkTarget {
  const raw = href.trim();
  if (raw.length === 0) {
    return { kind: 'ignore' };
  }
  if (raw.startsWith('#')) {
    return { kind: 'anchor', fragment: decode(raw.slice(1)) };
  }
  const scheme = schemeOf(raw);
  if (scheme !== null) {
    return EXTERNAL_SCHEMES.has(scheme) ? { kind: 'external', url: raw } : { kind: 'ignore' };
  }
  const hash = raw.indexOf('#');
  const pathPart = hash === -1 ? raw : raw.slice(0, hash);
  const fragment = hash === -1 ? null : decode(raw.slice(hash + 1));
  const path = decode(pathPart);
  if (/\.(md|markdown)$/i.test(path)) {
    return { kind: 'markdown', path, fragment };
  }
  return { kind: 'file', path };
}

function schemeOf(raw: string): string | null {
  const m = /^([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(raw);
  return m === null ? null : m[1].toLowerCase();
}

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}
