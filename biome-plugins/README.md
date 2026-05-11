# Biome GritQL plugins

Each `.grit` file in this directory enforces one rule from `.claude/rules/styling.md`.
Registered in `biome.json` under `plugins` using the **string array** form.

## Verified config shape — Biome 2.4.14

The `plugins` field accepts an array of strings (absolute or relative paths to `.grit` files).
The object form `{ "path": "...", "includes": [...] }` is **not** supported in this version:
the JSON schema defines `PluginConfiguration` as `{ "anyOf": [{ "type": "string" }] }` only.

Empirically confirmed by running a throwaway probe:

```jsonc
// biome.json excerpt
{
  "plugins": ["./biome-plugins/<file>.grit"]
}
```

The probe fired the expected diagnostic, confirming string-array form is the correct and only
supported shape. Downstream tasks must use this form.

## Example registration

```jsonc
{
  "plugins": [
    "./biome-plugins/no-inline-style.grit",
    "./biome-plugins/no-cx-string-literal.grit"
  ]
}
```

Note: `plugins` sits at the top level of `biome.json`, alongside `linter`, `formatter`, etc.
There is no `includes` filtering at registration time — scope the `.grit` pattern itself to the
file types you need (e.g. `language js` covers `.js`, `.jsx`, `.ts`, `.tsx`).
