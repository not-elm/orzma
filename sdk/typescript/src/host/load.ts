import type { PluginDescriptor } from './descriptors.ts';
import {
  type ApiImporter,
  type LoadedPlugin,
  loadPlugin,
  type MergeResult,
  mergeApis,
} from './plugin-loader.ts';

/**
 * Loads every api file of every plugin (in the given order) via the injected
 * importer and merges them. Fail-soft: a file that fails to import or validate is
 * recorded as a warning and skipped, so one broken plugin never disables the
 * others in the single host process. The caller's order — user plugins first —
 * drives first-wins on namespace collisions; the warning label is
 * `"<plugin> (<path>)"` so an intra-plugin collision is legible.
 */
export async function loadHostApi(
  plugins: PluginDescriptor[],
  importer: ApiImporter,
): Promise<MergeResult> {
  const units: LoadedPlugin[] = [];
  const loadWarnings: string[] = [];
  for (const plugin of plugins) {
    for (const apiPath of plugin.apiPaths) {
      try {
        units.push(await loadPlugin(`${plugin.name} (${apiPath})`, apiPath, importer));
      } catch (e) {
        loadWarnings.push(
          `plugin "${plugin.name}" api file ${apiPath} failed to load: ${e instanceof Error ? e.message : String(e)}`,
        );
      }
    }
  }
  const merged = mergeApis(units);
  return { api: merged.api, warnings: [...loadWarnings, ...merged.warnings] };
}
