# Styling Rules

Frontend code (`daemon/frontend`, future `extensions/*` and `sdk` React) uses
utility-first Tailwind v4 with the semantic token system defined in
`daemon/frontend/src/styles/theme.css`.

## Required

- Use semantic Tailwind utilities only: `bg-background`, `text-destructive`,
  `border-border`, `font-mono`, etc.
- Read `theme.css` for the full token list. New tokens go there, not into
  components.

## Forbidden

| Pattern | Example | Why |
| --- | --- | --- |
| Inline style props | `<div style={{ color: 'red' }}>` | Bypasses the token system; cannot be themed |
| Tailwind arbitrary values | `className="bg-[#abc] min-h-[200px]"` | Ad-hoc values drift from the palette |
| Raw palette vars | `var(--tn-blue)` in TSX or CSS | Layer 1 is private; consume Layer 2 (`--color-*`) instead |

## Need a new token?

Edit `daemon/frontend/src/styles/theme.css`:

1. Add a Layer 1 value if a new raw color is needed (`--tn-<name>`).
2. Map it to a Layer 2 semantic name under `@theme inline` (`--color-<name>`).
3. Use the generated Tailwind utility (e.g. `bg-<name>`) in components.

## Escape hatches

When physics demands it (xterm.js sizing, third-party DOM ref measurements,
ANSI color values from terminal state, etc.):

```tsx
// biome-ignore lint/plugin: xterm renderer requires direct CSS sizing
<div style={{ width: cols * cellWidth }} />
```

Real justification required. PR reviewers will reject vague reasons like
"needed it" or "easier this way".

## Enforcement

These rules are enforced by Biome GritQL plugins in `biome-plugins/`. Run
`pnpm lint` locally or `make fix-lint` to check. CI runs `pnpm lint:ci`.

## Existing legitimate exceptions

- `daemon/frontend/src/showcase/PaneMockup.tsx` — mockup needs literal pane
  proportions; suppressed inline with `biome-ignore lint/plugin`.
- `daemon/frontend/src/showcase/ColorSection.tsx` — showcase metadata
  reflects raw palette by design; suppressed file-wide via top-of-file
  `biome-ignore-all lint/plugin`.
- `daemon/frontend/src/styles/theme.css` — defines the tokens; suppressed
  file-wide via top-of-file `biome-ignore-all lint/plugin`.
