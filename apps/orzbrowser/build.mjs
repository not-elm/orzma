import { build } from 'esbuild';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, 'assets');

await mkdir(out, { recursive: true });

const result = await build({
  entryPoints: [join(here, 'web', 'main.ts')],
  bundle: true,
  format: 'iife',
  minify: true,
  write: false,
});
const script = result.outputFiles[0].text.replaceAll('</script', '<\\/script');
const html = await readFile(join(here, 'web', 'index.html'), 'utf8');
// NOTE: pass a function, not a string, as the replacement: a replacement string
// expands `$&` and `$'`, which minified code can contain, and would corrupt it.
await writeFile(
  join(out, 'chrome.html'),
  html.replace('<!-- BUNDLE -->', () => `<script>${script}</script>`),
);
console.log('orzbrowser chrome page written to assets/chrome.html');

const pageScript = await build({
  entryPoints: [join(here, 'web', 'page', 'main.ts')],
  bundle: true,
  format: 'iife',
  minify: true,
  legalComments: 'eof',
  write: false,
});
await writeFile(join(out, 'page.js'), pageScript.outputFiles[0].text);
console.log('orzbrowser page script written to assets/page.js');
