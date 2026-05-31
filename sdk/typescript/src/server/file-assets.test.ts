import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { fileAssetHandler } from './file-assets.ts';

describe('fileAssetHandler', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ozmux-assets-'));
  fs.writeFileSync(path.join(root, 'index.html'), '<h1>hi</h1>');
  fs.mkdirSync(path.join(root, 'dist'), { recursive: true });
  fs.writeFileSync(path.join(root, 'dist', 'sdk.js'), 'export const x=1;');
  const handler = fileAssetHandler(root);
  afterAll(() => {
    fs.rmSync(root, { recursive: true, force: true });
  });

  it('serves index.html with 200 + text/html', async () => {
    const r = await handler('index.html');
    expect(r.status).toBe(200);
    expect(r.contentType).toContain('text/html');
    expect(r.body.toString()).toBe('<h1>hi</h1>');
  });

  it('serves nested dist/sdk.js with a js mime type', async () => {
    const r = await handler('dist/sdk.js');
    expect(r.status).toBe(200);
    expect(r.contentType).toMatch(/javascript/);
    expect(r.body.toString()).toContain('export const x');
  });

  it('defaults empty path to index.html', async () => {
    const r = await handler('');
    expect(r.status).toBe(200);
    expect(r.body.toString()).toBe('<h1>hi</h1>');
  });

  it('404s a missing file', async () => {
    const r = await handler('nope.js');
    expect(r.status).toBe(404);
  });

  it('rejects path traversal with 403', async () => {
    const r = await handler('../secret');
    expect(r.status).toBe(403);
  });

  it('rejects an absolute path with 403', async () => {
    const r = await handler('/etc/passwd');
    expect(r.status).toBe(403);
  });
});
