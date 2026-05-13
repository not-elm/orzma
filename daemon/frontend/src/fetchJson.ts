/**
 * Fetches JSON from a URL and throws with a contextual error message on
 * non-2xx responses. The thrown error includes the URL, status code, and
 * status text so callers can `console.warn(e)` and produce useful logs.
 */
export async function fetchJson(url: string): Promise<unknown> {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: ${r.status} ${r.statusText}`);
  return r.json();
}
