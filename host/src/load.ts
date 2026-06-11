import type { ExtensionDescriptor } from './descriptors.ts';
import {
  type ApiImporter,
  type LoadedExtension,
  loadExtension,
  type MergeResult,
  mergeApis,
} from './extension-loader.ts';

/**
 * Loads every api file of every extension (in the given order) via the injected
 * importer and merges them. Fail-soft: a file that fails to import or validate is
 * recorded as a warning and skipped, so one broken extension never disables the
 * others in the single host process. The caller's order — user extensions first —
 * drives first-wins on namespace collisions; the warning label is
 * `"<extension> (<path>)"` so an intra-extension collision is legible.
 */
export async function loadHostApi(
  extensions: ExtensionDescriptor[],
  importer: ApiImporter,
): Promise<MergeResult> {
  const units: LoadedExtension[] = [];
  const loadWarnings: string[] = [];
  for (const extension of extensions) {
    for (const apiPath of extension.apiPaths) {
      try {
        units.push(await loadExtension(`${extension.name} (${apiPath})`, apiPath, importer));
      } catch (e) {
        loadWarnings.push(
          `extension "${extension.name}" api file ${apiPath} failed to load: ${e instanceof Error ? e.message : String(e)}`,
        );
      }
    }
  }
  const merged = mergeApis(units);
  return { api: merged.api, warnings: [...loadWarnings, ...merged.warnings] };
}
