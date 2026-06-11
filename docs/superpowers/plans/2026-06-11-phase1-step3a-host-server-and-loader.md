# Phase 1 — Step 3a: Host RPC Server + Multi-File Loader + Descriptor Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the runnable `@ozmux/sdk/host` — the descriptor handoff contract Rust will produce, a multi-file API loader composing Step 2's `loadPlugin`, a UDS RPC server that symmetrically decodes args + dispatches + encodes results, and the `node` host entry that ties env → load → bind → ready — all as isolated TypeScript, vitest-tested with real Unix sockets, with NO Rust and zero impact on the running app.

**Architecture (refined per design Q&A + spec-review):** Rust owns plugin discovery + `ozmux.toml` parsing + trust data; it resolves each plugin's manifest-declared `api = [...]` files to absolute paths and writes a descriptor handoff (`{ plugins: [{ name, apiPaths[], assetRoot }] }`). The host is a dumb executor: read the descriptors, `import()` each api path (no `api.ts` hardcoding — the manifest names the file(s), multiple allowed), `mergeApis` across all loaded units **user-plugins-first**, bind the RPC server, then signal readiness via a **filesystem marker** (matching the existing Rust poll). The RPC server decodes `{__u8}` args before dispatch (symmetric with result-encode), bounds inbound framing, replies with an error frame to any `reqId`-addressable malformed call, and never re-checks capabilities (Rust-side, Step 4).

**Tech Stack:** TypeScript (strict, nodenext, verbatimModuleSyntax), `zod` (already a workspace dep) for the descriptor schema, vitest with real `node:net` UDS sockets, `node:fs/promises`. Biome.

---

## Where this fits

Steps 1 ✅, 2 ✅. **Step 3a (this doc)** = runnable host (TS, isolated). **Step 3b** (next) = Rust: extend `PluginManifest` with the plugin-level `api: Vec<String>` field, plugin discovery (user-first) → write the descriptor JSON, parse `ozmux.toml` → `ViewRegistry` with capabilities + `entry`/`id` validation, reshape `ExtensionManagerPlugin` to spawn exactly one host, **asset `{plugin,path}` protocol + `OZMUX_HOST_ASSET_SOCK`**. Step 4 = webview host-API bridge. Step 5 = remove old machinery. Step 6 = memo plugin migration + E2E.

### 3a → 3b handoff contract (frozen here)
- **Env vars Step 3b must set when spawning `node main.ts`:** `OZMUX_HOST_RPC_SOCK` (RPC UDS path), `OZMUX_HOST_MANIFEST` (path to the descriptor JSON file Rust writes), `OZMUX_HOST_READY_PATH` (path the host `touch`es after binding — Rust polls its existence).
- **Descriptor JSON shape:** `{ "plugins": [{ "name": string, "apiPaths": string[] (absolute), "assetRoot": string (absolute) }] }`. Rust resolves the manifest `api = [...]` entries to absolute paths and is responsible for path-traversal + plugin-name safety validation (the host trusts the descriptor — it is the trust boundary, not the host).
- **Readiness = filesystem marker** (NOT a stdout line): chosen because the current Rust lifecycle already polls `<path>/.ready` existence (`crates/extension_host/src/command.rs:174-177`) and only pipes stdin (`:151-161`). 3b points that poll at `OZMUX_HOST_READY_PATH`; no stdout-reader plumbing is added.
- **Deferred to 3b (intentional, not an oversight):** asset serving. `assetRoot` is carried in the descriptor *now* but **unconsumed in 3a**; the asset server + its `OZMUX_HOST_ASSET_SOCK` env + the `{plugin,path}` request reshape (design spec §④) all land in 3b. Until then, OSC-mounting a view yields a blank webview — expected on the feature branch.

Spec: `docs/superpowers/specs/2026-06-11-phase1-single-host-process-design.md` (§②, §③). Conventions: `.claude/rules/typescript.md` (JSDoc on exports; comments TODO/NOTE/biome-ignore only; `.ts` import extensions; `import type`/`export type`). Run tests from `sdk/typescript/` (`pnpm test`), lint from repo root (`pnpm lint`).

---

## File Structure

| File | Responsibility | Action |
| --- | --- | --- |
| `sdk/typescript/src/host/descriptors.ts` | zod schema → `PluginDescriptor`/`HostManifest` types + `parseHostManifest` | Create |
| `sdk/typescript/src/host/load.ts` | `loadHostApi(plugins, importer)` — multi-file, user-first, **fail-soft** merge | Create |
| `sdk/typescript/src/host/rpc-server.ts` | `bindHostRpcServer(sockPath, api)` — bounded UDS NDJSON RPC, symmetric arg-decode, error-frame on malformed | Create |
| `sdk/typescript/src/host/main.ts` | `node` host entry: env → load → bind → ready-file | Create |
| `sdk/typescript/src/host/index.ts` | barrel re-exports | Modify |
| `*.test.ts` next to each | vitest | Create |

---

## Task 1: descriptor contract (`descriptors.ts`, zod)

**Files:** Create `sdk/typescript/src/host/descriptors.ts` + `descriptors.test.ts`; modify `index.ts`.

- [ ] **Step 1: Write the failing test** — `descriptors.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { parseHostManifest } from './descriptors.ts';

describe('parseHostManifest', () => {
  it('parses a well-formed manifest', () => {
    const m = parseHostManifest(
      JSON.stringify({
        plugins: [{ name: 'memo', apiPaths: ['/abs/memo/api/fs.ts'], assetRoot: '/abs/memo' }],
      }),
    );
    expect(m.plugins).toEqual([
      { name: 'memo', apiPaths: ['/abs/memo/api/fs.ts'], assetRoot: '/abs/memo' },
    ]);
  });

  it('accepts an empty plugins array', () => {
    expect(parseHostManifest('{"plugins":[]}').plugins).toEqual([]);
  });

  it('throws on malformed JSON', () => {
    expect(() => parseHostManifest('{not json')).toThrow(/host manifest/i);
  });

  it('throws when plugins is missing or not an array', () => {
    expect(() => parseHostManifest('{}')).toThrow(/host manifest/i);
    expect(() => parseHostManifest('{"plugins":"x"}')).toThrow(/host manifest/i);
  });

  it('throws when a plugin entry has the wrong shape', () => {
    expect(() => parseHostManifest('{"plugins":[{"name":"x"}]}')).toThrow(/host manifest/i);
    expect(() =>
      parseHostManifest('{"plugins":[{"name":"x","apiPaths":"y","assetRoot":"z"}]}'),
    ).toThrow(/host manifest/i);
  });
});
```

- [ ] **Step 2: Run, expect fail** — from `sdk/typescript/`: `pnpm test descriptors` → FAIL (module not found).

- [ ] **Step 3: Implement** — `sdk/typescript/src/host/descriptors.ts`:

```ts
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
```

> The plugin name / path safety validation lives Rust-side (it produces the descriptor); the host schema only checks structural shape. `z.infer` makes the schema the single source of truth for `HostManifest`/`PluginDescriptor`.

- [ ] **Step 4: Re-export** — append to `index.ts`:

```ts
export { parseHostManifest } from './descriptors.ts';
export type { HostManifest, PluginDescriptor } from './descriptors.ts';
```

- [ ] **Step 5: Verify** — `pnpm test descriptors` → PASS (5 tests); `pnpm check-types` clean; repo-root `pnpm lint` clean (confirm `zod` import resolves — it is in `sdk/typescript/package.json` deps as `"zod": "catalog:"`).

- [ ] **Step 6: Commit**

```bash
git add sdk/typescript/src/host/descriptors.ts sdk/typescript/src/host/descriptors.test.ts sdk/typescript/src/host/index.ts
git commit -m "feat(sdk/host): add zod host-manifest descriptor contract"
```

---

## Task 2: multi-file API loader (`load.ts`, fail-soft)

**Files:** Create `sdk/typescript/src/host/load.ts` + `load.test.ts`; modify `index.ts`.

- [ ] **Step 1: Write the failing test** — `load.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import type { PluginDescriptor } from './descriptors.ts';
import { loadHostApi } from './load.ts';
import type { ApiImporter } from './plugin-loader.ts';

function fakeImporter(modules: Record<string, unknown>): ApiImporter {
  return async (specifier: string) => {
    if (!(specifier in modules)) throw new Error(`no module ${specifier}`);
    return { default: modules[specifier] };
  };
}

const d = (name: string, apiPaths: string[]): PluginDescriptor => ({ name, apiPaths, assetRoot: `/p/${name}` });

describe('loadHostApi', () => {
  it('merges multiple api files within one plugin', async () => {
    const importer = fakeImporter({
      '/a/fs.ts': { fs: { read: async () => 'r' } },
      '/a/net.ts': { net: { get: async () => 'g' } },
    });
    const { api, warnings } = await loadHostApi([d('a', ['/a/fs.ts', '/a/net.ts'])], importer);
    expect(Object.keys(api).sort()).toEqual(['fs', 'net']);
    expect(warnings).toEqual([]);
  });

  it('keeps the first loader on namespace collision (user-first order) and warns', async () => {
    const userFs = { read: async () => 'user' };
    const importer = fakeImporter({
      '/user/fs.ts': { fs: userFs },
      '/bundled/fs.ts': { fs: { read: async () => 'bundled' } },
    });
    const { api, warnings } = await loadHostApi(
      [d('user', ['/user/fs.ts']), d('bundled', ['/bundled/fs.ts'])],
      importer,
    );
    expect(api.fs).toBe(userFs);
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toContain('fs');
  });

  it('is fail-soft: a broken api file is skipped with a warning, others still load', async () => {
    const importer = fakeImporter({
      '/a/bad.ts': 42, // non-object default → loadPlugin throws
      '/a/ok.ts': { fs: { read: async () => 'r' } },
    });
    const { api, warnings } = await loadHostApi([d('a', ['/a/bad.ts', '/a/ok.ts'])], importer);
    expect(Object.keys(api)).toEqual(['fs']);
    expect(warnings.some((w) => w.includes('/a/bad.ts'))).toBe(true);
  });

  it('is fail-soft: a missing module is skipped, not fatal', async () => {
    const importer = fakeImporter({ '/a/ok.ts': { fs: { read: async () => 1 } } });
    const { api, warnings } = await loadHostApi([d('a', ['/a/missing.ts', '/a/ok.ts'])], importer);
    expect(Object.keys(api)).toEqual(['fs']);
    expect(warnings.some((w) => w.includes('/a/missing.ts'))).toBe(true);
  });
});
```

- [ ] **Step 2: Run, expect fail** — `pnpm test load` → FAIL.

- [ ] **Step 3: Implement** — `sdk/typescript/src/host/load.ts`:

```ts
import type { PluginDescriptor } from './descriptors.ts';
import { type ApiImporter, type MergeResult, loadPlugin, mergeApis } from './plugin-loader.ts';

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
  const units = [];
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
```

- [ ] **Step 4: Re-export** — append to `index.ts`:

```ts
export { loadHostApi } from './load.ts';
```

- [ ] **Step 5: Verify** — `pnpm test load` → PASS (4 tests); `pnpm check-types` clean.

- [ ] **Step 6: Commit**

```bash
git add sdk/typescript/src/host/load.ts sdk/typescript/src/host/load.test.ts sdk/typescript/src/host/index.ts
git commit -m "feat(sdk/host): add fail-soft multi-file user-first api loader"
```

---

## Task 3: RPC socket server (`rpc-server.ts`)

**Files:** Create `sdk/typescript/src/host/rpc-server.ts` + `rpc-server.test.ts`; modify `index.ts`.

- [ ] **Step 1: Write the failing test** — `rpc-server.test.ts` (real UDS, mirrors `handlers-server.test.ts` style):

```ts
import * as fs from 'node:fs/promises';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { ApiNamespaceMap } from './define-api.ts';
import { bindHostRpcServer } from './rpc-server.ts';

let server: net.Server | undefined;
let sockPath = '';

const api: ApiNamespaceMap = {
  fs: {
    read: async (p: string) => `contents:${p}`,
    size: async (bytes: Uint8Array) => bytes.length,
  },
};

beforeEach(async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'ozmux-host-'));
  sockPath = path.join(dir, 'host.rpc.sock');
});

afterEach(async () => {
  if (server) {
    await new Promise<void>((res) => server?.close(() => res()));
    server = undefined;
  }
});

function connect(): Promise<net.Socket> {
  return new Promise((resolve, reject) => {
    const s = net.connect(sockPath);
    s.once('connect', () => resolve(s));
    s.once('error', reject);
  });
}

function rpc(s: net.Socket, frame: unknown): Promise<Record<string, unknown>> {
  return new Promise((resolve) => {
    let buf = '';
    s.on('data', (chunk) => {
      buf += chunk.toString('utf8');
      const idx = buf.indexOf('\n');
      if (idx !== -1) resolve(JSON.parse(buf.slice(0, idx)));
    });
    s.write(`${JSON.stringify(frame)}\n`);
  });
}

describe('bindHostRpcServer', () => {
  it('chmods the socket 0600 and sets maxConnections', async () => {
    server = await bindHostRpcServer(sockPath, api);
    expect((await fs.stat(sockPath)).mode & 0o777).toBe(0o600);
    expect(server.maxConnections).toBe(64);
  });

  it('dispatches a call and returns an ok frame', async () => {
    server = await bindHostRpcServer(sockPath, api);
    const s = await connect();
    const r = await rpc(s, { reqId: '1', ns: 'fs', method: 'read', args: ['/x'] });
    expect(r).toEqual({ reqId: '1', ok: true, value: 'contents:/x' });
    s.destroy();
  });

  it('decodes a {__u8} binary arg before dispatch', async () => {
    server = await bindHostRpcServer(sockPath, api);
    const s = await connect();
    const r = await rpc(s, {
      reqId: '2',
      ns: 'fs',
      method: 'size',
      args: [{ __u8: Buffer.from([1, 2, 3]).toString('base64') }],
    });
    expect(r).toEqual({ reqId: '2', ok: true, value: 3 });
    s.destroy();
  });

  it('returns an error frame for an unknown method', async () => {
    server = await bindHostRpcServer(sockPath, api);
    const s = await connect();
    const r = await rpc(s, { reqId: '3', ns: 'fs', method: 'ghost', args: [] });
    expect(r.ok).toBe(false);
    expect(r.error).toBe('unknown method fs.ghost');
    s.destroy();
  });

  it('returns an error frame for a reqId-addressable but malformed frame (no hang)', async () => {
    server = await bindHostRpcServer(sockPath, api);
    const s = await connect();
    const r = await rpc(s, { reqId: '4', ns: 'fs' }); // missing method/args
    expect(r.reqId).toBe('4');
    expect(r.ok).toBe(false);
    s.destroy();
  });
});
```

- [ ] **Step 2: Run, expect fail** — `pnpm test rpc-server` → FAIL.

- [ ] **Step 3: Implement** — `sdk/typescript/src/host/rpc-server.ts`:

```ts
import * as fs from 'node:fs/promises';
import * as net from 'node:net';
import { decodeHostValue } from './binary-codec.ts';
import type { ApiNamespaceMap } from './define-api.ts';
import { type HostCallFrame, dispatchHostCall } from './dispatch.ts';

const MAX_RPC_LINE_BYTES = 8 * 1024 * 1024;

/**
 * Binds a Unix-socket NDJSON RPC server that dispatches `{reqId, ns, method, args}`
 * frames against `api`. Binary `{__u8}` args are decoded to `Uint8Array` before
 * dispatch (symmetric with the result-encode path). A `reqId`-addressable but
 * malformed frame gets an error reply (never a silent drop → no caller hang); a
 * wholly-unparseable line (no `reqId`) is dropped. The trusted surface identity
 * and capability check live Rust-side; this server does not re-check them.
 */
export async function bindHostRpcServer(sockPath: string, api: ApiNamespaceMap): Promise<net.Server> {
  await fs.unlink(sockPath).catch(() => {});
  const server = net.createServer(onConnection);
  server.maxConnections = 64;
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(sockPath, () => {
      server.off('error', reject);
      resolve();
    });
  });
  await fs.chmod(sockPath, 0o600);
  return server;

  function onConnection(conn: net.Socket): void {
    let buf = '';
    conn.on('data', (chunk) => {
      buf += chunk.toString('utf8');
      if (buf.length > MAX_RPC_LINE_BYTES) {
        // NOTE: an unframed flood (bytes with no newline) would grow buf without
        // bound; cap it and drop the connection rather than risk OOM on the one
        // host process. TODO: surface a structured error before destroy.
        conn.destroy();
        return;
      }
      let idx = buf.indexOf('\n');
      while (idx !== -1) {
        const line = buf.slice(0, idx);
        buf = buf.slice(idx + 1);
        handleLine(conn, line);
        idx = buf.indexOf('\n');
      }
    });
    conn.on('error', () => {});
  }

  function handleLine(conn: net.Socket, line: string): void {
    let raw: unknown;
    try {
      raw = JSON.parse(line);
    } catch {
      return;
    }
    const { reqId, ns, method, args } = raw as Partial<HostCallFrame>;
    if (typeof reqId !== 'string' || typeof ns !== 'string' || typeof method !== 'string' || !Array.isArray(args)) {
      if (typeof reqId === 'string') {
        conn.write(`${JSON.stringify({ reqId, ok: false, error: 'malformed host call frame' })}\n`);
      }
      return;
    }
    const frame: HostCallFrame = { reqId, ns, method, args: args.map(decodeHostValue) };
    dispatchHostCall(api, frame)
      .then((result) => conn.write(`${JSON.stringify(result)}\n`))
      .catch((err) => {
        console.error('host rpc: dispatch threw', err);
      });
  }
}
```

- [ ] **Step 4: Re-export** — append to `index.ts`:

```ts
export { bindHostRpcServer } from './rpc-server.ts';
```

- [ ] **Step 5: Verify** — `pnpm test rpc-server` → PASS (5 tests); `pnpm test host` → all host tests green; `pnpm check-types` clean; repo-root `pnpm lint` clean over `src/host/**`.

- [ ] **Step 6: Commit**

```bash
git add sdk/typescript/src/host/rpc-server.ts sdk/typescript/src/host/rpc-server.test.ts sdk/typescript/src/host/index.ts
git commit -m "feat(sdk/host): add bounded UDS RPC server with symmetric arg decode"
```

---

## Task 4: `node` host entry (`main.ts`)

**Files:** Create `sdk/typescript/src/host/main.ts` + `main.test.ts`.

> `main.ts` is the executable Step 3b spawns as `node <main.ts>`. The testable logic (reading env + the descriptor file) is the exported `readHostStartup`; the socket bind + ready-file write are glue gated behind `import.meta.main` (verified to be `false` when vitest imports the module on Node v24, so the test does NOT boot a server).

- [ ] **Step 1: Write the failing test** — `main.test.ts`:

```ts
import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { readHostStartup } from './main.ts';

let dir = '';
beforeEach(async () => {
  dir = await fs.mkdtemp(path.join(os.tmpdir(), 'ozmux-main-'));
});
afterEach(async () => {
  await fs.rm(dir, { recursive: true, force: true });
});

describe('readHostStartup', () => {
  it('reads the rpc sock path, ready path, and parsed manifest from env', async () => {
    const manifestPath = path.join(dir, 'host.json');
    await fs.writeFile(
      manifestPath,
      JSON.stringify({ plugins: [{ name: 'memo', apiPaths: ['/abs/a.ts'], assetRoot: '/abs' }] }),
    );
    const startup = await readHostStartup({
      OZMUX_HOST_RPC_SOCK: '/tmp/x.sock',
      OZMUX_HOST_READY_PATH: '/tmp/x.ready',
      OZMUX_HOST_MANIFEST: manifestPath,
    });
    expect(startup.rpcSockPath).toBe('/tmp/x.sock');
    expect(startup.readyPath).toBe('/tmp/x.ready');
    expect(startup.manifest.plugins[0].name).toBe('memo');
  });

  it('throws naming each missing required env var', async () => {
    await expect(readHostStartup({ OZMUX_HOST_READY_PATH: '/r', OZMUX_HOST_MANIFEST: '/m' })).rejects.toThrow(
      /OZMUX_HOST_RPC_SOCK/,
    );
    await expect(readHostStartup({ OZMUX_HOST_RPC_SOCK: '/s', OZMUX_HOST_MANIFEST: '/m' })).rejects.toThrow(
      /OZMUX_HOST_READY_PATH/,
    );
    await expect(readHostStartup({ OZMUX_HOST_RPC_SOCK: '/s', OZMUX_HOST_READY_PATH: '/r' })).rejects.toThrow(
      /OZMUX_HOST_MANIFEST/,
    );
  });
});
```

- [ ] **Step 2: Run, expect fail** — `pnpm test main` → FAIL.

- [ ] **Step 3: Implement** — `sdk/typescript/src/host/main.ts`:

```ts
import * as fs from 'node:fs/promises';
import { type HostManifest, parseHostManifest } from './descriptors.ts';
import { loadHostApi } from './load.ts';
import { bindHostRpcServer } from './rpc-server.ts';

/** Resolved host startup inputs read from the environment. */
export interface HostStartup {
  rpcSockPath: string;
  readyPath: string;
  manifest: HostManifest;
}

/** Reads + validates the host's startup inputs from `env`. Throws naming the first missing/invalid var. */
export async function readHostStartup(env: Record<string, string | undefined>): Promise<HostStartup> {
  const rpcSockPath = env.OZMUX_HOST_RPC_SOCK;
  if (!rpcSockPath) throw new Error('missing env OZMUX_HOST_RPC_SOCK');
  const readyPath = env.OZMUX_HOST_READY_PATH;
  if (!readyPath) throw new Error('missing env OZMUX_HOST_READY_PATH');
  const manifestPath = env.OZMUX_HOST_MANIFEST;
  if (!manifestPath) throw new Error('missing env OZMUX_HOST_MANIFEST');
  const manifest = parseHostManifest(await fs.readFile(manifestPath, 'utf8'));
  return { rpcSockPath, readyPath, manifest };
}

async function main(): Promise<void> {
  const { rpcSockPath, readyPath, manifest } = await readHostStartup(process.env);
  const { api, warnings } = await loadHostApi(manifest.plugins, (s) => import(s));
  for (const w of warnings) console.error(`host: ${w}`);
  await bindHostRpcServer(rpcSockPath, api);
  // NOTE: readiness is a FILE written ONLY after the RPC socket is listening, so
  // Rust's existing `<path>/.ready` existence-poll (command.rs) observes a host
  // that is actually ready. Writing it before bind would race the first call.
  await fs.writeFile(readyPath, '');
}

if (import.meta.main) {
  main().catch((err) => {
    console.error('host: fatal', err);
    process.exit(1);
  });
}
```

> `import.meta.main` is recognized by `@types/node` (`ImportMeta.main: boolean`) under this repo's `nodenext`/`verbatimModuleSyntax` tsconfig and is `false` when vitest imports the module — verified during spec-review. No fallback guard is needed.

- [ ] **Step 4: Verify** — `pnpm test main` → PASS (2 tests). `pnpm test host` → ALL host tests green. `pnpm check-types` clean. Repo-root `pnpm lint` clean over `src/host/**`. Confirm importing `main.ts` in the test does NOT bind a server or hang (the boot guard works).

- [ ] **Step 5: Commit**

```bash
git add sdk/typescript/src/host/main.ts sdk/typescript/src/host/main.test.ts
git commit -m "feat(sdk/host): add node host entry (env -> load -> bind -> ready file)"
```

---

## Done criteria for Step 3a

- `pnpm test host` green across all `src/host/*.test.ts` (Step 2's suites + 3a's descriptors / load / rpc-server / main).
- `pnpm check-types` clean; `pnpm lint` clean over `src/host/**`.
- Importing `main.ts` does not start a server (boot guard verified); the ready file is written only after the RPC socket binds.
- The RPC server: bounds inbound line size + `maxConnections`, decodes `{__u8}` args, and replies with an error frame (never a silent drop) to any `reqId`-addressable malformed call.
- `loadHostApi` is fail-soft (one broken api file → warning + skip, not a dead host).
- No Rust touched; no existing SDK module affected; the host is runnable but not yet spawned (Step 3b wires it).

After 3a lands, Step 3b is authored against the Rust internals (`command.rs` spawn/env/readiness-poll, `host.rs` RuntimeRoot/EndpointRegistry, `scheme.rs`/`protocol.rs`, `extension_manager.rs`, `main.rs`, `configs/`): extend `PluginManifest` with `api: Vec<String>`, discover plugins user-first, write the `HostManifest` JSON + the `OZMUX_HOST_{RPC_SOCK,MANIFEST,READY_PATH}` env + spawn one `node main.ts` (pointing the existing `.ready` poll at `OZMUX_HOST_READY_PATH`), parse `ozmux.toml` → `ViewRegistry` (caps + `entry`/`id` validation), and the asset `{plugin,path}` protocol + `OZMUX_HOST_ASSET_SOCK`.

### Step 3b carry-forward (from Step 3a final integration review)
- **JSON keys are camelCase:** the Rust descriptor writer must emit `apiPaths`/`assetRoot` exactly (serde `#[serde(rename_all = "camelCase")]` or explicit renames) to match the zod schema.
- **`apiPaths` + `assetRoot` must be absolute** when Rust writes them; path-traversal + plugin-name safety validation lives Rust-side (the host trusts the descriptor).
- **Readiness:** repoint the existing `command.rs` `.ready` existence-poll at `OZMUX_HOST_READY_PATH` (no new mechanism); the host writes that file only after the RPC socket binds.
- **User-first ordering:** push `~/.config/ozmux/plugins/*` before `<repo>/plugins/*` in the descriptor `plugins` array (`loadHostApi` is already user-first by input order) — an intentional reversal of `extension_manager.rs`'s current bundled-first order.
- **Step 4 (bridge) note:** the host's RPC `.catch` now replies with an `internal host error` frame; the Rust↔host relay should still tolerate a missing reply per `reqId` (timeout/cleanup) so a dead host never wedges a webview Promise.

## Status: Step 3a COMPLETE (2026-06-11)

All 4 tasks landed, each through an independent spec+quality review; final integration review: READY. Commits `b16a3bc`, `568c30b`, `e5a285c`, `e703594`, plus review-follow-up `fe8e5fe`. Evidence: `@ozmux/sdk` host suite **38/38** (8 files), `pnpm check-types` clean, `pnpm lint` clean over `src/host/**`. Isolated TS — no Rust touched, zero impact on the running app. Two review-caught defects fixed mid-step: an `Array` default-export hole in `loadPlugin` (code-review) and a `JSON.parse('null')` host-crash in the RPC server (integration review).
