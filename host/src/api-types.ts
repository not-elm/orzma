/** A single host-API method. `never[]` params let any concrete method shape fit (contravariance). */
export type ApiMethod = (...args: never[]) => unknown;

/** A nested map of host-API namespaces to their methods (an extension's `api.ts` default-export shape). */
export type ApiNamespaceMap = Record<string, Record<string, ApiMethod>>;
