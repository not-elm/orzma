/** A single host-API method. `never[]` params let any concrete method shape fit (contravariance). */
export type ApiMethod = (...args: never[]) => unknown;

/** A nested map of host-API namespaces to their methods (the `api.ts` default-export shape). */
export type ApiNamespaceMap = Record<string, Record<string, ApiMethod>>;

/**
 * Identity helper that preserves the literal type of a plugin's host API so
 * `export default defineApi({...})` keeps precise per-method types. Zero runtime
 * cost — it returns its argument unchanged.
 */
export function defineApi<const T extends ApiNamespaceMap>(api: T): T {
  return api;
}
