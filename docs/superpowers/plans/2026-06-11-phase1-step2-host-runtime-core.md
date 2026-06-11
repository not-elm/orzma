# Phase 1 — Step 2: `@ozmux/host` Runtime Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the pure-logic core of the single Node host runtime as a new `@ozmux/host` package export — `defineApi`, the `{__u8}` binary codec, the extension loader (namespace merge, first-wins + warn), and the RPC dispatcher — all dependency-injected and unit-tested, with NO sockets, NO process bootstrap, and zero impact on the running app.

**Architecture:** A new `host/src/` module. `defineApi` is a zero-cost identity helper preserving extension API literal types. `extension-loader` merges per-extension default-exported API objects into one namespace map (globally-unique namespaces; earlier extension wins on collision, later one warned). `dispatch` invokes `api[ns][method](...args)` and encodes a top-level binary result via the `{__u8: base64}` envelope. Everything is a pure/async function over plain values; the socket server, the `node` host entry, and the Rust spawn/wiring come in Step 3.

**Tech Stack:** TypeScript (strict, `nodenext`, `verbatimModuleSyntax`), vitest, Node `Buffer` for base64. Biome for lint/format.

---

## Where this fits (Phase 1 sequence)

Step 1 ✅ (capability foundation, Rust). **Step 2 (this doc)** = host runtime pure core (TS). Step 3 = socket server + `node` host entry + reshape `ExtensionManagerPlugin` to spawn one host + scan extension roots → `ViewRegistry` (with the `entry`/`id` validation carried from Step 1) + asset `{extension,path}` protocol. Step 4 = host-API bridge (Proxy injection, `cef.emit`, Rust capability gate on `GrantedNamespaces`, return path). Step 5 = remove old machinery. Step 6 = memo extension migration + `extensions/*` root + E2E.

Spec: `docs/superpowers/specs/2026-06-11-phase1-single-host-process-design.md` (§②, §③).

> **Conventions (.claude/rules/typescript.md):** JSDoc on every `export` whose meaning isn't obvious; comment taxonomy `// TODO:` / `// NOTE:` / `// biome-ignore <rule>: <reason>` only; named imports with `.ts` extensions (repo uses `allowImportingTsExtensions` + `verbatimModuleSyntax`); minimize export surface (these exports ARE the `@ozmux/host` public API, consumed by Step 3's host entry + the Rust-spawned process). A package-entry `index.ts` barrel is the established pattern (`src/server/index.ts`, `src/surface/index.ts`).
> **Run from `host/`:** tests `pnpm test` (vitest); types `pnpm check-types`. Lint from repo root: `pnpm lint` (biome over `host/**`). All must be clean before each commit.

---

## File Structure

| File | Responsibility | Action |
| --- | --- | --- |
| `host/src/define-api.ts` | `defineApi` identity helper + `ApiNamespaceMap`/`ApiMethod` types | Create |
| `host/src/binary-codec.ts` | `{__u8}` base64 envelope encode/decode (boundary-tagged) | Create |
| `host/src/extension-loader.ts` | `mergeApis` (first-wins + warn) + `loadExtension` (injected importer) | Create |
| `host/src/dispatch.ts` | `dispatchHostCall(api, frame)` → result frame | Create |
| `host/src/index.ts` | Package-entry barrel for `@ozmux/host` | Create |
| `host/package.json` | Declare the `@ozmux/host` package entry | Modify |
| `*.test.ts` next to each module | vitest suites | Create |

---

## Task 1: `defineApi` + types, and the `@ozmux/host` package barrel

**Files:**
- Create: `host/src/define-api.ts`
- Create: `host/src/define-api.test.ts`
- Create: `host/src/index.ts`
- Modify: `host/package.json`

- [ ] **Step 1: Write the failing test** — `host/src/define-api.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { defineApi } from './define-api.ts';

describe('defineApi', () => {
  it('returns the same object reference (identity helper)', () => {
    const api = { fs: { read: async (p: string) => p } };
    expect(defineApi(api)).toBe(api);
  });

  it('preserves nested methods callable form', async () => {
    const api = defineApi({ math: { add: async (a: number, b: number) => a + b } });
    expect(await api.math.add(2, 3)).toBe(5);
  });
});
```

- [ ] **Step 2: Run it, expect failure**

Run (from `host/`): `pnpm test define-api`
Expected: FAIL — cannot find module `./define-api.ts`.

- [ ] **Step 3: Implement** — `host/src/define-api.ts`:

```ts
/** A single host-API method. `never[]` params let any concrete method shape fit (contravariance). */
export type ApiMethod = (...args: never[]) => unknown;

/** A nested map of host-API namespaces to their methods (the `api.ts` default-export shape). */
export type ApiNamespaceMap = Record<string, Record<string, ApiMethod>>;

/**
 * Identity helper that preserves the literal type of an extension's host API so
 * `export default defineApi({...})` keeps precise per-method types. Zero runtime
 * cost — it returns its argument unchanged.
 */
export function defineApi<const T extends ApiNamespaceMap>(api: T): T {
  return api;
}
```

- [ ] **Step 4: Create the package barrel** — `host/src/index.ts`:

```ts
export { defineApi } from './define-api.ts';
export type { ApiMethod, ApiNamespaceMap } from './define-api.ts';
```

- [ ] **Step 5: Wire the package entry in `host/package.json`**

Point the `@ozmux/host` package entry at the barrel:

```json
    "exports": {
      ".": {
        "types": "./src/index.ts",
        "default": "./src/index.ts"
      }
    },
```

- [ ] **Step 6: Run test + types**

Run (from `host/`): `pnpm test define-api` → PASS (2 tests).
Run: `pnpm check-types` → no errors.

- [ ] **Step 7: Commit**

```bash
git add host/src/define-api.ts host/src/define-api.test.ts host/src/index.ts host/package.json
git commit -m "feat(host): add defineApi identity helper and @ozmux/host package entry"
```

---

## Task 2: `{__u8}` binary codec (boundary-tagged)

**Files:**
- Create: `host/src/binary-codec.ts`
- Create: `host/src/binary-codec.test.ts`
- Modify: `host/src/index.ts` (re-export)

- [ ] **Step 1: Write the failing test** — `host/src/binary-codec.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { decodeHostValue, encodeHostValue, isBinaryEnvelope } from './binary-codec.ts';

describe('binary-codec', () => {
  it('wraps a top-level Uint8Array as a base64 envelope', () => {
    const enc = encodeHostValue(new Uint8Array([1, 2, 3]));
    expect(isBinaryEnvelope(enc)).toBe(true);
    expect((enc as { __u8: string }).__u8).toBe(Buffer.from([1, 2, 3]).toString('base64'));
  });

  it('wraps a Node Buffer (Uint8Array subclass)', () => {
    const enc = encodeHostValue(Buffer.from('hi', 'utf8'));
    expect(isBinaryEnvelope(enc)).toBe(true);
  });

  it('passes plain JSON values through unchanged', () => {
    expect(encodeHostValue({ a: 1 })).toEqual({ a: 1 });
    expect(encodeHostValue('x')).toBe('x');
    expect(encodeHostValue(42)).toBe(42);
    expect(encodeHostValue(null)).toBe(null);
  });

  it('round-trips through decode back to a Uint8Array', () => {
    const original = new Uint8Array([9, 8, 7]);
    const decoded = decodeHostValue(encodeHostValue(original));
    expect(decoded).toBeInstanceOf(Uint8Array);
    expect(Array.from(decoded as Uint8Array)).toEqual([9, 8, 7]);
  });

  it('decode passes non-envelope values through', () => {
    expect(decodeHostValue({ a: 1 })).toEqual({ a: 1 });
    expect(decodeHostValue('x')).toBe('x');
  });

  it('does NOT deep-encode a nested Uint8Array (boundary-tagged only)', () => {
    const enc = encodeHostValue({ buf: new Uint8Array([1]) }) as { buf: unknown };
    expect(isBinaryEnvelope(enc.buf)).toBe(false);
  });
});
```

- [ ] **Step 2: Run it, expect failure**

Run: `pnpm test binary-codec`
Expected: FAIL — cannot find module `./binary-codec.ts`.

- [ ] **Step 3: Implement** — `host/src/binary-codec.ts`:

```ts
/** Wire form of a binary value crossing the JSON-string IPC channel. */
export interface BinaryEnvelope {
  __u8: string;
}

/** Narrows an unknown wire value to a `BinaryEnvelope`. */
export function isBinaryEnvelope(value: unknown): value is BinaryEnvelope {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as { __u8?: unknown }).__u8 === 'string'
  );
}

/**
 * Encodes a host method's return value for the JSON-string channel. A top-level
 * `Uint8Array`/`Buffer` becomes a base64 `BinaryEnvelope`; every other value
 * passes through unchanged. Boundary-tagged: nested binary is NOT walked.
 */
export function encodeHostValue(value: unknown): unknown {
  if (value instanceof Uint8Array) {
    return { __u8: Buffer.from(value).toString('base64') } satisfies BinaryEnvelope;
  }
  return value;
}

/** Reverses `encodeHostValue`: a `BinaryEnvelope` becomes a `Uint8Array`. */
export function decodeHostValue(value: unknown): unknown {
  if (isBinaryEnvelope(value)) {
    return new Uint8Array(Buffer.from(value.__u8, 'base64'));
  }
  return value;
}
```

- [ ] **Step 4: Re-export from the barrel** — append to `host/src/index.ts`:

```ts
export { decodeHostValue, encodeHostValue, isBinaryEnvelope } from './binary-codec.ts';
export type { BinaryEnvelope } from './binary-codec.ts';
```

- [ ] **Step 5: Run test + types**

Run: `pnpm test binary-codec` → PASS (6 tests). `pnpm check-types` → clean.

- [ ] **Step 6: Commit**

```bash
git add host/src/binary-codec.ts host/src/binary-codec.test.ts host/src/index.ts
git commit -m "feat(host): add boundary-tagged {__u8} binary codec"
```

---

## Task 3: extension loader (merge first-wins + warn, injected importer)

**Files:**
- Create: `host/src/extension-loader.ts`
- Create: `host/src/extension-loader.test.ts`
- Modify: `host/src/index.ts` (re-export)

- [ ] **Step 1: Write the failing test** — `host/src/extension-loader.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { loadExtension, mergeApis } from './extension-loader.ts';

describe('mergeApis', () => {
  it('merges disjoint namespaces from multiple extensions', () => {
    const { api, warnings } = mergeApis([
      { name: 'a', api: { fs: { read: async () => 1 } } },
      { name: 'b', api: { net: { get: async () => 2 } } },
    ]);
    expect(Object.keys(api).sort()).toEqual(['fs', 'net']);
    expect(warnings).toEqual([]);
  });

  it('keeps the earlier extension on namespace collision and warns for the later', () => {
    const first = { read: async () => 'first' };
    const { api, warnings } = mergeApis([
      { name: 'a', api: { fs: first } },
      { name: 'b', api: { fs: { read: async () => 'second' } } },
    ]);
    expect(api.fs).toBe(first);
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toContain('fs');
    expect(warnings[0]).toContain('b');
    expect(warnings[0]).toContain('a');
  });
});

describe('loadExtension', () => {
  it('returns the default export as the extension api', async () => {
    const importer = async () => ({ default: { fs: { read: async () => 'ok' } } });
    const p = await loadExtension('memo', '/abs/memo/api.ts', importer);
    expect(p.name).toBe('memo');
    expect(Object.keys(p.api)).toEqual(['fs']);
  });

  it('rejects when the module has no object default export', async () => {
    const importer = async () => ({ default: 42 });
    await expect(loadExtension('bad', '/abs/bad/api.ts', importer)).rejects.toThrow(/default-export an object/);
  });

  it('rejects when the module has no default export', async () => {
    const importer = async () => ({});
    await expect(loadExtension('bad', '/abs/bad/api.ts', importer)).rejects.toThrow(/default-export an object/);
  });
});
```

- [ ] **Step 2: Run it, expect failure**

Run: `pnpm test extension-loader` → FAIL (module not found).

- [ ] **Step 3: Implement** — `host/src/extension-loader.ts`:

```ts
import type { ApiNamespaceMap } from './define-api.ts';

/** A single extension's loaded API, keyed by extension name for collision reporting. */
export interface LoadedExtension {
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
 * Merges extension APIs into one namespace map. Namespaces are globally unique; on
 * collision the earlier extension (by input order) wins and a warning is recorded
 * for the later one. Callers pass extensions in a deterministic order (sorted dir
 * name) so first-wins is stable.
 */
export function mergeApis(extensions: LoadedExtension[]): MergeResult {
  const api: ApiNamespaceMap = {};
  const owner: Record<string, string> = {};
  const warnings: string[] = [];
  for (const extension of extensions) {
    for (const ns of Object.keys(extension.api)) {
      if (ns in api) {
        warnings.push(
          `namespace "${ns}" from extension "${extension.name}" ignored; already provided by "${owner[ns]}"`,
        );
        continue;
      }
      api[ns] = extension.api[ns];
      owner[ns] = extension.name;
    }
  }
  return { api, warnings };
}

/**
 * Loads one extension's `api.ts` default export via the injected importer. Throws
 * when the module does not default-export an object.
 */
export async function loadExtension(
  name: string,
  apiPath: string,
  importer: ApiImporter,
): Promise<LoadedExtension> {
  const mod = await importer(apiPath);
  const def = mod.default;
  if (def === null || typeof def !== 'object') {
    throw new Error(`extension "${name}" api.ts must default-export an object`);
  }
  return { name, api: def as ApiNamespaceMap };
}
```

- [ ] **Step 4: Re-export from the barrel** — append to `host/src/index.ts`:

```ts
export { loadExtension, mergeApis } from './extension-loader.ts';
export type { ApiImporter, LoadedExtension, MergeResult } from './extension-loader.ts';
```

- [ ] **Step 5: Run test + types**

Run: `pnpm test extension-loader` → PASS (5 tests). `pnpm check-types` → clean.

- [ ] **Step 6: Commit**

```bash
git add host/src/extension-loader.ts host/src/extension-loader.test.ts host/src/index.ts
git commit -m "feat(host): add extension loader with first-wins namespace merge"
```

---

## Task 4: RPC dispatcher

**Files:**
- Create: `host/src/dispatch.ts`
- Create: `host/src/dispatch.test.ts`
- Modify: `host/src/index.ts` (re-export)

- [ ] **Step 1: Write the failing test** — `host/src/dispatch.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { dispatchHostCall } from './dispatch.ts';
import type { ApiNamespaceMap } from './define-api.ts';

const api: ApiNamespaceMap = {
  fs: {
    read: async (path: string) => `contents:${path}`,
    bytes: async () => new Uint8Array([1, 2]),
    boom: async () => {
      throw new Error('explode');
    },
  },
};

describe('dispatchHostCall', () => {
  it('invokes api[ns][method](...args) and returns an ok frame', async () => {
    const r = await dispatchHostCall(api, { reqId: '1', ns: 'fs', method: 'read', args: ['/x'] });
    expect(r).toEqual({ reqId: '1', ok: true, value: 'contents:/x' });
  });

  it('encodes a binary result as a {__u8} envelope', async () => {
    const r = await dispatchHostCall(api, { reqId: '2', ns: 'fs', method: 'bytes', args: [] });
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value).toEqual({ __u8: Buffer.from([1, 2]).toString('base64') });
  });

  it('returns an error frame for an unknown namespace', async () => {
    const r = await dispatchHostCall(api, { reqId: '3', ns: 'ghost', method: 'x', args: [] });
    expect(r).toEqual({ reqId: '3', ok: false, error: 'unknown method ghost.x' });
  });

  it('returns an error frame for an unknown method', async () => {
    const r = await dispatchHostCall(api, { reqId: '4', ns: 'fs', method: 'nope', args: [] });
    expect(r).toEqual({ reqId: '4', ok: false, error: 'unknown method fs.nope' });
  });

  it('returns an error frame when the method throws', async () => {
    const r = await dispatchHostCall(api, { reqId: '5', ns: 'fs', method: 'boom', args: [] });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toContain('explode');
  });
});
```

- [ ] **Step 2: Run it, expect failure**

Run: `pnpm test dispatch` → FAIL (module not found).

- [ ] **Step 3: Implement** — `host/src/dispatch.ts`:

```ts
import { encodeHostValue } from './binary-codec.ts';
import type { ApiNamespaceMap } from './define-api.ts';

/** A host call as it arrives off the wire (the trusted surface identity lives Rust-side, not here). */
export interface HostCallFrame {
  reqId: string;
  ns: string;
  method: string;
  args: unknown[];
}

/** The dispatcher's reply: success with an (already binary-encoded) value, or an error. */
export type HostResultFrame =
  | { reqId: string; ok: true; value: unknown }
  | { reqId: string; ok: false; error: string };

/**
 * Dispatches a host call to `api[ns][method](...args)`, encoding a binary result
 * via `encodeHostValue`. An unknown namespace/method or a thrown method produces
 * an error frame; this never throws.
 */
export async function dispatchHostCall(
  api: ApiNamespaceMap,
  frame: HostCallFrame,
): Promise<HostResultFrame> {
  const fn = api[frame.ns]?.[frame.method];
  if (typeof fn !== 'function') {
    return { reqId: frame.reqId, ok: false, error: `unknown method ${frame.ns}.${frame.method}` };
  }
  try {
    const value = await (fn as unknown as (...a: unknown[]) => unknown)(...frame.args);
    return { reqId: frame.reqId, ok: true, value: encodeHostValue(value) };
  } catch (e) {
    return { reqId: frame.reqId, ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}
```

- [ ] **Step 4: Re-export from the barrel** — append to `host/src/index.ts`:

```ts
export { dispatchHostCall } from './dispatch.ts';
export type { HostCallFrame, HostResultFrame } from './dispatch.ts';
```

- [ ] **Step 5: Run the whole host suite + types + lint**

Run (from `host/`): `pnpm test host` → PASS (all host tests: 2 + 6 + 5 + 5 = 18). `pnpm check-types` → clean.
Run (from repo root): `pnpm lint` → biome clean over the new files (fix with `pnpm lint:fix` if needed, then re-run).

- [ ] **Step 6: Commit**

```bash
git add host/src/dispatch.ts host/src/dispatch.test.ts host/src/index.ts
git commit -m "feat(host): add RPC dispatcher invoking api[ns][method]"
```

---

## Done criteria for Step 2

- `pnpm --filter @ozmux/host test` (or `pnpm test` in `host/`) green for all `src/*.test.ts` (18 tests).
- `pnpm check-types` clean; `pnpm lint` (biome) clean over `host/src/**`.
- No existing SDK test affected; no runtime/Rust behavior changed (pure new module, not yet imported anywhere).
- `@ozmux/host` exports `defineApi`, `ApiNamespaceMap`/`ApiMethod`, the binary codec, `mergeApis`/`loadExtension`, and `dispatchHostCall`.

After this step lands, Step 3 adds the socket server (`bindHostRpcServer`), the `node` host entry (env-wired extension discovery using `loadExtension` + native `import()`), reshapes `ExtensionManagerPlugin` to spawn exactly one host, scans extension roots → `ViewRegistry` (with `entry` path-traversal + `id` validation carried from Step 1), and extends the asset `Request` to `{extension, path}`.

## Carry into Step 3 (from Step 2 final integration review)

1. **Real importer:** pass `(s) => import(s)` directly as the `ApiImporter` — Node native `import()` of an absolute `api.ts` path matches the `(specifier) => Promise<{ default?: unknown }>` shape exactly.
2. **User-extensions-first ordering (intentional reversal):** the array passed to `mergeApis` must be `[...userExtensions, ...bundledExtensions]` so user extensions win first-wins collisions. This reverses the current `extension_manager.rs` bundled-first order — mark the scan site with a `// NOTE:`.
3. **Symmetric arg decode (do NOT drop):** `dispatchHostCall` spreads `...frame.args` raw. The Step 3 socket server must run `decodeHostValue` over each incoming `frame.args` element ( `{__u8}` → `Uint8Array`) BEFORE calling `dispatchHostCall`, mirroring the result-encode path. This is a transport-layer concern, correctly outside the pure dispatcher.
4. **Max response-size guard:** enforce the spec's §③ size limit in the socket server, after `JSON.stringify(resultFrame)` — keep the pure dispatcher free of transport concerns. A `// TODO:` at that site is appropriate until the threshold is fixed.
5. **`value` typing:** when the socket layer `JSON.stringify`s the result frame, consider tightening `HostResultFrame.value` (currently `unknown`) to a JSON-value alias for compile-time assurance.

## Status: Step 2 COMPLETE (2026-06-11)

All 4 tasks landed, each through an independent spec+quality review; final integration review: READY. Commits `a0ec819`, `344b34d`, `365bb3e`, `6a602ef`. Evidence: `@ozmux/host` host suite **20/20** (4 files), `pnpm check-types` clean, `pnpm lint` clean over `host/src/**`. Pure new module — not yet imported anywhere; zero impact on the running app. Two review-caught defects fixed mid-step: the binary-codec `{__u8}` collision caveat (documented), and a real `mergeApis` prototype-inheritance bug (`Object.create(null)`, regression-tested).
