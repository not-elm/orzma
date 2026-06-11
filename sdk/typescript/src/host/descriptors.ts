import { z } from 'zod';

const pluginDescriptorSchema = z.object({
  name: z.string(),
  apiPaths: z.array(z.string()),
  assetRoot: z.string(),
});

const hostManifestSchema = z.object({
  plugins: z.array(pluginDescriptorSchema),
});

/** One plugin's load + serve descriptor, produced by Rust and consumed by the host. */
export type PluginDescriptor = z.infer<typeof pluginDescriptorSchema>;

/** The handoff Rust writes (referenced by `OZMUX_HOST_MANIFEST`) and the host reads at startup. */
export type HostManifest = z.infer<typeof hostManifestSchema>;

/** Parses + validates the host-manifest JSON. Throws with a `host manifest` message on any malformed shape. */
export function parseHostManifest(json: string): HostManifest {
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch (e) {
    throw new Error(`invalid host manifest JSON: ${e instanceof Error ? e.message : String(e)}`);
  }
  const result = hostManifestSchema.safeParse(raw);
  if (!result.success) {
    throw new Error(`invalid host manifest: ${result.error.message}`);
  }
  return result.data;
}
