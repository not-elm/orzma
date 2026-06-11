import type { ApiNamespaceMap } from './define-api.ts';

/** A single plugin's loaded API, keyed by plugin name for collision reporting. */
export interface LoadedPlugin {
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
 * Merges plugin APIs into one namespace map. Namespaces are globally unique; on
 * collision the earlier plugin (by input order) wins and a warning is recorded
 * for the later one. Callers pass plugins in a deterministic order (sorted dir
 * name) so first-wins is stable.
 */
export function mergeApis(plugins: LoadedPlugin[]): MergeResult {
  const api: ApiNamespaceMap = Object.create(null);
  const owner: Record<string, string> = Object.create(null);
  const warnings: string[] = [];
  for (const plugin of plugins) {
    for (const ns of Object.keys(plugin.api)) {
      if (ns in api) {
        warnings.push(
          `namespace "${ns}" from plugin "${plugin.name}" ignored; already provided by "${owner[ns]}"`,
        );
        continue;
      }
      api[ns] = plugin.api[ns];
      owner[ns] = plugin.name;
    }
  }
  return { api, warnings };
}

/**
 * Loads one plugin's `api.ts` default export via the injected importer. Throws
 * when the module does not default-export an object.
 */
export async function loadPlugin(
  name: string,
  apiPath: string,
  importer: ApiImporter,
): Promise<LoadedPlugin> {
  const mod = await importer(apiPath);
  const def = mod.default;
  if (def === null || typeof def !== 'object') {
    throw new Error(`plugin "${name}" api.ts must default-export an object`);
  }
  return { name, api: def as ApiNamespaceMap };
}
