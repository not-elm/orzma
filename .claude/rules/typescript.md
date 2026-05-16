# TypeScript Coding Rules

TypeScript code in this repo (`daemon/frontend`, `sdk/*`, `extensions/*`,
`tools/*.ts`, and `biome-plugins/`) follows the rules below. These rules
complement [`.claude/rules/styling.md`](styling.md) (Tailwind / theme
tokens) and the conventions in `CLAUDE.md`. They mirror the spirit of
[`.claude/rules/rust.md`](rust.md) for Rust — same taste, translated.

## Comments

Non-JSDoc line comments are restricted. The only permitted forms:

| Form | Use |
| --- | --- |
| `// TODO: <text>` | Work to address later |
| `// NOTE: <text>` | A non-obvious invariant, race, or warning to the reader |
| `// biome-ignore <rule>: <text>` | Required justification on every biome suppression (already enforced) |
| `// @ts-expect-error <text>` / `// @ts-ignore <text>` | Required justification on every TypeScript suppression |

Forbidden:

| Pattern | Example | Why |
| --- | --- | --- |
| Plain narrative comments | `// increments counter` | What the code does belongs in identifiers |
| Block comments | `/* ... */` (non-JSDoc) | Same |
| Commented-out code | `// const x = oldImpl();` | History lives in git |
| `// NOTE:` with no concrete non-obvious info | `// NOTE: this is the handler` | Promote to identifier rename or delete |

Note: `/** ... */` JSDoc is **not** a "line comment" for this rule — see the next section.

The principle is the same as `rust.md`: if a comment is just restating
what the code does, the code or its names need to do the work instead.
A `// NOTE:` earns its keep only when it captures something a careful
reader would otherwise miss (e.g. a race, a workaround for a specific
upstream bug, an invariant that the surrounding code relies on).

## JSDoc

Required:

| Place | Style |
| --- | --- |
| Every `export` (function, class, interface, type, const) | `/** ... */` — one-line summary, blank line, optional body |

Not required (but recommended):

- Module-level summaries (`/** @file ... */`) — unusual in this codebase; skip unless the file has a non-obvious responsibility.
- Internal (non-exported) helpers — keep names good instead.
- Test files (`*.test.ts(x)`) and test helpers (`__test-helpers.ts`).
- Re-export-only files (`index.ts` with nothing but `export { ... }`).

Style guide:

- The first line is a noun phrase or third-person verb phrase, matching the Rust convention. Stay descriptive, not imperative.
- Document parameters only when their meaning is non-obvious from the type/name. Don't write `@param name - The name.`.
- Document return values only when their meaning is non-obvious or when the function has side effects worth flagging.
- For React hooks, document side effects (effect dependencies, cleanup behavior, subscriptions) in the body.

Forbidden:

| Pattern | Why |
| --- | --- |
| `export` item with no JSDoc when the meaning is non-obvious from the name | Public API owes the reader an explanation |
| Placeholder JSDoc like `/** TODO: write this */` | Don't ship empty docs |
| `@param` / `@returns` that just restate the TypeScript type | Redundant; TS already says it |

## Visibility — minimize export surface

TypeScript has no `pub(crate)` equivalent. The analog is `export` (visible to other modules) vs unexported (file-local). Reach for unexported by default.

Required:

- A symbol that's only used in its defining file must not be `export`ed.
- A symbol that's only used inside one parent directory should live in a file that lives next to its callers, not in a shared barrel.
- Test files may freely import any symbol they need; if they need a non-exported symbol, **add a test-only export** (e.g. `export const __test_only = ...`) rather than widening the production surface.

Forbidden:

| Pattern | Why |
| --- | --- |
| Barrel `index.ts` files that re-export everything | Inflates the surface, hides what's actually used externally |
| `export` on items with no out-of-file callers | Same as `pub` with no cross-crate callers in Rust |

Recommended workflow when adding a new item:

1. Start without `export`.
2. If a sibling file needs it, add `export`.
3. If multiple sibling files need it through a barrel, ask whether the barrel is earning its keep.

## Imports

Biome enforces import ordering. The rules here are the ones biome
does NOT enforce:

- Prefer **named imports** over namespace imports (`import * as foo`)
  unless the namespace genuinely earns its keep (e.g. shadcn-style
  primitives). Namespace imports hide what's actually used.
- No wildcard re-exports (`export * from './foo'`) outside curated
  prelude / shadcn-registry files.

## Escape hatches

TypeScript's escape hatches require justification, the same way Rust's
`#[expect(..., reason = "...")]` does:

```ts
// biome-ignore lint/correctness/useExhaustiveDependencies: liveWid is load-bearing — the effect must re-run after the live container actually mounts
useEffect(...);

// @ts-expect-error vite-singlefile types lag the runtime API
import vitePlugin from 'vite-plugin-singlefile';
```

- Always include a one-line reason. "Easier this way" or "the type is wrong" is not a reason — name the upstream bug, the specific tool limitation, or the non-obvious invariant that justifies the suppression.
- Prefer `@ts-expect-error` (fails if the underlying error stops happening) over `@ts-ignore` (silently absorbs whatever).
- Prefer the smallest scope possible: a single-line suppression over a file-level `biome-ignore-all`.

## Tests

Test files (`*.test.ts(x)`) have a loosened rule set:

- `// NOTE:` is not required for inline setup helpers, mocks, fakes — names should still carry meaning.
- Inline narrative comments **explaining what the test exercises** are acceptable when the assertion alone doesn't make the scenario obvious. But "step-by-step" running commentary is still discouraged — favor splitting into `describe` / `it` blocks instead.
- JSDoc on test helpers is not required.

## Enforcement

Tool-enforced (biome):

- Import ordering
- `biome-ignore` requires a reason string (existing rule)
- Inline-style / arbitrary Tailwind value bans (see `styling.md`)
- Run via `pnpm lint` / `pnpm lint:fix` / `make fix-lint`

Not tool-enforced — review-time check required. The following rules cannot
be detected by biome and must be checked manually during code review (and
by Claude when proposing changes):

- Comment taxonomy — only `// TODO:` / `// NOTE:` / `// biome-ignore` / `// @ts-expect-error` (each with a reason)
- JSDoc requirement on `export`s with non-obvious meaning
- `export` visibility minimization
- Justification quality on `biome-ignore` and `@ts-expect-error`
- "No commented-out code"

If you add a biome plugin or other tooling that detects any of these, move
the corresponding entry into the tool-enforced list above.

## Existing legitimate exceptions

- `daemon/frontend/src/test-setup.ts` — jsdom shim setup uses block comments to delimit stub sections. Pre-existing; permitted as long as the surrounding context is non-trivial.
- `daemon/frontend/src/vendor.d.ts` — third-party type re-export notes. Pre-existing; permitted.
- `daemon/frontend/src/showcase/**` — design-system showcase files; comment requirements relaxed in line with the styling.md exception for the same files.

(Append entries here as they are discovered, with a brief justification.)
