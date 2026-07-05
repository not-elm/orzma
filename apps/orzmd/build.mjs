import { build } from 'esbuild';
import { copyFile, mkdir, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, 'assets');

await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });

// The full clean above wipes the checked-in placeholders, so restore them: the
// build output must stay gitignored while the empty assets/ dir is preserved.
await writeFile(join(out, '.gitignore'), '*\n!.gitkeep\n!.gitignore\n');
await writeFile(join(out, '.gitkeep'), '');

await build({
  entryPoints: [join(here, 'web', 'main.ts')],
  bundle: true,
  format: 'esm',
  splitting: true,
  outdir: out,
  entryNames: 'bundle',
  chunkNames: 'chunks/[name]-[hash]',
  loader: { '.woff2': 'file', '.woff': 'file', '.ttf': 'file' },
  assetNames: 'fonts/[name]-[hash]',
  minify: true,
});

await copyFile(join(here, 'web', 'index.html'), join(out, 'index.html'));
console.log('orzmd web bundle written to assets/');
