/** Extracts the lowercased URL scheme of `raw`, or `null` if it has none. */
export function schemeOf(raw: string): string | null {
  const m = /^([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(raw);
  return m === null ? null : m[1].toLowerCase();
}

/** Percent-decodes `s`, falling back to the raw string on malformed escapes. */
export function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}
