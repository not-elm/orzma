import type { ApiNamespaceMap } from './api-types.ts';

/** A single extension's loaded API, keyed by extension name for collision reporting. */
export interface LoadedExtension {
  name: string;
  api: ApiNamespaceMap;
}

/** The merged host API plus any collision warnings to log. */
export interface MergeResult {
  api: ApiNamespaceMap;
  warnings: string[];
}

/** Imports a module by specifier; injected so loading is testable without disk. */
export type ApiImporter = (specifier: string) => Promise<{ default?: unknown }>;

/**
 * Merges extension APIs into one namespace map. Namespaces are globally unique; on
 * collision the earlier extension (by input order) wins and a warning is recorded
 * for the later one. Callers pass extensions in a deterministic order (sorted dir
 * name) so first-wins is stable.
 */
export function mergeApis(extensions: LoadedExtension[]): MergeResult {
  const api: ApiNamespaceMap = Object.create(null);
  const owner: Record<string, string> = Object.create(null);
  const warnings: string[] = [];
  for (const extension of extensions) {
    for (const ns of Object.keys(extension.api)) {
      if (ns in api) {
        warnings.push(
          `namespace "${ns}" from extension "${extension.name}" ignored; already provided by "${owner[ns]}"`,
        );
        continue;
      }
      api[ns] = extension.api[ns];
      owner[ns] = extension.name;
    }
  }
  return { api, warnings };
}

/**
 * Loads one extension's `api.ts` default export via the injected importer. Throws
 * when the module does not default-export a namespace object (an array is
 * rejected too, since `typeof [] === 'object'` would otherwise slip through and
 * `mergeApis` would register numeric-index "namespaces").
 */
export async function loadExtension(
  name: string,
  apiPath: string,
  importer: ApiImporter,
): Promise<LoadedExtension> {
  const mod = await importer(apiPath);
  const def = mod.default;
  if (def === null || typeof def !== 'object' || Array.isArray(def)) {
    throw new Error(`extension "${name}" api.ts must default-export an object`);
  }
  return { name, api: def as ApiNamespaceMap };
}
