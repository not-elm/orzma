import * as fs from 'node:fs/promises';
import * as path from 'node:path';
import type { AssetHandler, AssetResponse } from './asset-server.ts';

const MIME: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.wasm': 'application/wasm',
};

/**
 * Builds an {@link AssetHandler} that serves files from `root`. Empty path →
 * `index.html`. Path traversal outside `root` is rejected with 403; missing
 * files return 404.
 */
export function fileAssetHandler(root: string): AssetHandler {
  const base = path.resolve(root);
  return async (reqPath: string): Promise<AssetResponse> => {
    const rel = reqPath === '' ? 'index.html' : reqPath;
    const resolved = path.resolve(base, rel);
    if (resolved !== base && !resolved.startsWith(base + path.sep)) {
      return { status: 403, contentType: 'text/plain', body: 'forbidden' };
    }
    try {
      const body = await fs.readFile(resolved);
      const ext = path.extname(resolved).toLowerCase();
      return { status: 200, contentType: MIME[ext] ?? 'application/octet-stream', body };
    } catch {
      return { status: 404, contentType: 'text/plain', body: 'not found' };
    }
  };
}
