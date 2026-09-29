import { build } from 'esbuild';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, 'assets');

async function bundle(...entry) {
  const result = await build({
    entryPoints: [join(here, 'web', ...entry)],
    bundle: true,
    format: 'iife',
    minify: true,
    write: false,
  });
  return result.outputFiles[0].text;
}

await mkdir(out, { recursive: true });

const script = (await bundle('main.ts')).replaceAll('</script', '<\\/script');
const html = await readFile(join(here, 'web', 'index.html'), 'utf8');
// NOTE: pass a function, not a string, as the replacement: a replacement string
// expands `$&` and `$'`, which minified code can contain, and would corrupt it.
await writeFile(
  join(out, 'chrome.html'),
  html.replace('<!-- BUNDLE -->', () => `<script>${script}</script>`),
);
console.log('orzbrowser chrome page written to assets/chrome.html');

await writeFile(join(out, 'page.js'), await bundle('page', 'main.ts'));
console.log('orzbrowser page script written to assets/page.js');
