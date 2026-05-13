# Rust Coding Rules

Rust code in this repo (`cli/`, `daemon/*`) follows the rules below. Edition 2024, toolchain pinned to 1.95. These rules complement [`.claude/rules/styling.md`](styling.md) (frontend) and the conventions documented in `CLAUDE.md`.

## Module layout — no `mod.rs`

Required:

| Pattern | Example | Why |
| --- | --- | --- |
| Rust 2018+ module files | `foo.rs` + `foo/bar.rs` | One file declares the module; no ambiguity about which file is the module root |

Forbidden:

| Pattern | Example | Why |
| --- | --- | --- |
| `mod.rs` as module root | `foo/mod.rs` | Hard to navigate (many files all named `mod.rs`); editor tabs look identical |

## Comments

Non-doc line comments are restricted. The only permitted forms:

| Form | Use |
| --- | --- |
| `// TODO: <text>` | Work to address later |
| `// NOTE: <text>` | A non-obvious invariant or warning to the reader |
| `// SAFETY: <text>` | Required justification for any `unsafe { ... }` block (rustc / clippy idiom) |

Forbidden:

| Pattern | Example | Why |
| --- | --- | --- |
| Plain narrative comments | `// increments counter` | What the code does belongs in identifiers |
| Block comments | `/* ... */` | Same |
| Commented-out code | `// let x = old_impl();` | History lives in git |

Note: `///` and `//!` are **doc comments**, not "line comments" for this rule — see the next section.

## Doc comments

Required:

| Place | Style |
| --- | --- |
| Every externally-public item (`pub` only — not `pub(crate)`, `pub(super)`, `pub(in path)`) | `///` — one-line summary, blank line, optional body |
| Each file-level module — `lib.rs`, `main.rs`, and every `foo.rs` that declares a module | `//!` — module-level purpose in 1–3 lines |

Equivalent attribute forms (`#[doc = "..."]`, `#[doc = include_str!("README.md")]`) count as doc comments and satisfy this rule.

Not required (but recommended):

- `pub(crate)` / `pub(super)` / `pub(in path)` items
- Inline modules (`mod inner { ... }` inside another file)
- `#[cfg(test)] mod tests { ... }` blocks and their contents

Style guide:

- The first line is a noun phrase or third-person singular verb phrase (e.g., `/// Returns the active pane.`). Stay descriptive, not imperative.
- Code examples use triple backticks; hidden setup lines may be prefixed with `# `.
- Invariants live under a `# Invariants` section.

Forbidden:

| Pattern | Why |
| --- | --- |
| Externally `pub` item with no doc | Public API owes the reader an explanation |
| Placeholder doc like `/// TODO: write this` | Don't ship empty docs |

## Imports

Required:

- Every `use` is declared at the top of the file (immediately after the `//!` if present).
- All `use` statements form a single contiguous block — no blank lines separating `std`, external crates, and crate-local imports.

Exception:

- Inside `#[cfg(test)] mod tests { ... }` blocks, locally-scoped `use` statements are allowed (e.g., `use super::*;`, test-only fixtures). Test code is the only place where locality outweighs the "all imports at the top" rule.

Forbidden:

| Pattern | Example | Why |
| --- | --- | --- |
| `use` inside non-test functions or blocks | `fn f() { use std::io; ... }` | Spreads scope across the file |
| Blank lines between `std` / external / crate `use`s | `use std::...;\n\nuse tokio::...;` | We do not group |
| Glob import in consumer code | `use foo::*;` | Hides which symbols are in scope |

Note on preludes: a module that *defines* a prelude (i.e., re-exports curated names for downstream consumers) may itself use `pub use foo::*;` inside its own definition. The rule above forbids glob imports in **consumer** code.

## Escape hatches

When a rule is physically impossible to follow (e.g., trybuild fixtures, generated code, FFI conventions), justify the exception with a one-line `// NOTE:` and apply a local lint allowance:

```rust
// NOTE: trybuild fixtures must be top-level files; module-layout rule doesn't apply here.
#[expect(clippy::needless_pass_by_value, reason = "trybuild fixture signature")]
```

- Prefer `#[expect(..., reason = "...")]` over `#[allow(...)]`. `#[expect]` fails the build if the underlying lint stops firing, which prevents stale allowances from accumulating.
- Fall back to `#[allow(...)]` only when `#[expect]` is impractical (e.g., conditionally compiled code where the lint may or may not trigger).

"Hard to read" or "annoying" is not a valid reason.

## Enforcement

Tool-enforced:

- `cargo clippy --fix --allow-dirty --allow-staged && cargo fmt`, or `make fix-lint`
- Add `#![warn(missing_docs)]` to each crate's `lib.rs` / `main.rs` to enforce the doc requirement (rollout tracked separately)
- CI runs the existing `clippy` and `fmt` checks

Not tool-enforced — review-time check required. The following rules cannot currently be detected by `clippy` / `rustfmt` and must be checked manually during code review (and by Claude when proposing changes):

- `mod.rs` ban
- Comment taxonomy — only `// TODO:` / `// NOTE:` / `// SAFETY:`
- File-level module `//!` requirement
- "No blank lines between import groups"
- `#[expect]` preference over `#[allow]`

If you add a tool or script that detects any of these, move the corresponding entry into the tool-enforced list above.

## Existing legitimate exceptions

- (None recorded yet — append entries here as they are discovered, with a brief justification.)
