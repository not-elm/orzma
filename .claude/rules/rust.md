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
| `// NOTE: <text>` | A **critical caveat** — only when overlooking it causes real harm; see the bar below |
| `// SAFETY: <text>` | Required justification for any `unsafe { ... }` block (rustc / clippy idiom) |

Forbidden:

| Pattern | Example | Why |
| --- | --- | --- |
| Plain narrative comments | `// increments counter` | What the code does belongs in identifiers |
| Block comments | `/* ... */` | Same |
| Commented-out code | `// let x = old_impl();` | History lives in git |
| `// NOTE:` for merely non-obvious or "good to know" info | `// NOTE: this is the handler` | NOTE is for critical caveats only — rename the identifier or delete |

A `// NOTE:` is reserved for a **critical caveat**: something that, if a
reader overlooks it, leads to a bug, a crash, data loss, a security
issue, or a violated invariant. "Non-obvious" or "good to know" is not
enough — the test is concrete harm on the line that misses it.
Qualifying examples: a race condition, a workaround for a specific
upstream bug, an ordering requirement the surrounding code silently
relies on, an invariant a later mutation must preserve. If overlooking
the comment causes no real failure, do not write it — rename an
identifier so the code carries the meaning, or delete it.

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

## Visibility — minimize scope

Every item (functions, types, fields, modules, constants, traits) starts
private and is widened only when a concrete caller outside the current
scope needs it. Reach for the narrowest visibility that compiles.

Ladder, from narrowest to widest — pick the first one that works:

| Visibility | Use when |
| --- | --- |
| (none — private) | Only used inside the defining module |
| `pub(super)` | Only used by the immediate parent module |
| `pub(in path)` | Used by a specific subtree of the crate |
| `pub(crate)` | Used elsewhere in this crate, but not exported |
| `pub` | Part of the crate's external API |

Required:

- Default to private. Add visibility modifiers only when a real caller
  forces it.
- **MANDATORY:** any item (regardless of current visibility) whose only
  callers live inside its defining module MUST be private (no
  visibility modifier). This applies symmetrically to `pub` items
  used only in one module, `pub(crate)` items used only in one
  module, `pub(super)` items used only in one module, and so on.
  Re-narrow during refactors, not just on the way up.
- For `pub` items used cross-module but not cross-crate, demoting to
  `pub(crate)` is **recommended but not required** — library crates
  may legitimately expose APIs that no in-workspace consumer
  references yet. Apply judgement based on whether the item is part
  of the crate's intended external API.
- Struct fields stay private unless an external constructor or pattern
  match requires them. Prefer accessor methods over `pub` fields.
- Helper modules used by only one parent should be declared inside that
  parent (`mod helper;` without `pub`) so the names cannot leak.

Exception — items inside a container whose own visibility is already
narrow:

- When the container struct, enum, or trait is `pub(crate)` (or
  narrower), associated methods and fields written as plain `pub` do not
  need to be demoted to match. The container already caps reachability,
  so `pub fn new()` on a `pub(crate) struct Foo` cannot be called from
  outside the crate regardless of the `pub` keyword on the method.
- Still demote when the associated item can go strictly **narrower than
  the container** (e.g., a helper method only called inside the
  defining module of a `pub(crate)` struct should be private). The
  exception buys you "don't bother matching", not "stop narrowing
  further".
- This applies symmetrically to struct fields, enum variants' inner
  items, and inherent + trait `impl` blocks.

Forbidden:

| Pattern | Why |
| --- | --- |
| Any visibility wider than private on items with no out-of-module callers | The item is only used inside its defining module; it must be private. Applies symmetrically to `pub`, `pub(crate)`, `pub(super)`, `pub(in path)` |
| `pub` fields on structs with invariants | Bypasses any validation in constructors / setters |
| `pub use` re-exports for items that no external consumer references | Same as above; widens the surface for no caller |

Not forbidden (but recommended to review):

| Pattern | Why it's not strict |
| --- | --- |
| `pub` on items with cross-module-but-not-cross-crate callers | Library crates may publish APIs for downstream consumers we don't see in this workspace. Demote to `pub(crate)` when you're confident the item is not part of the intended external surface; keep `pub` otherwise |

Recommended workflow when adding a new item:

1. Start with no visibility modifier (private).
2. Compile. If a caller in the same crate fails to resolve it, widen by
   one step (`pub(super)` → `pub(in path)` → `pub(crate)`).
3. Only reach `pub` when the item is genuinely part of the crate's
   external API (used by another workspace member or a downstream
   consumer).

Recommended workflow when reviewing existing code:

- **MANDATORY check:** for any item (any current visibility), grep for
  callers outside the defining module. If there are none, demote to
  private. This is non-negotiable; module-scoped items must be
  private.
- **Optional check:** for `pub` items with no cross-crate callers,
  demoting to `pub(crate)` is encouraged when you are confident the
  item is not part of the crate's intended external API. For library
  crates where the future-consumer set is open, keeping `pub` is
  acceptable.

Tooling note: `#![warn(unreachable_pub)]` catches `pub` items that
nothing outside the crate can reach. It is useful for one-off audits
of the *optional* `pub` → `pub(crate)` narrowing, but it does **not**
catch the *mandatory* "module-scoped items must be private" rule —
the lint only fires for items reachable from outside the crate, not
for items reachable from outside their module but still inside the
crate. For the mandatory rule, manual grep-based review is the only
tool today. Run `unreachable_pub` locally for crate-export audits,
then turn it back off — the container exception above would create
persistent noise.

## Item ordering — private items last

Within an `impl` block (and at module / file scope), declare items in
descending visibility order: `pub`, then `pub(crate)` / `pub(super)` /
`pub(in path)`, then private (no-modifier) items **last**. Private
helper functions live at the bottom of the block, below every item that
exposes API.

Rationale: a reader scanning an `impl` block or module sees the surface
it can call first; the implementation details that support that surface
come after.

Required:

- Private (no-visibility-modifier) `fn`s are declared after every `pub`
  and `pub(crate)`-or-narrower-but-still-exported item in the same
  `impl` block or module.
- Within the private group, keep related helpers together; order them
  for readability (roughly call order).

Not constrained:

- `#[cfg(test)] mod tests { ... }` contents — test code is exempt.
- Trait `impl` blocks whose method order is dictated by the trait.
- Struct field order — governed by layout / grouping concerns, not this
  rule.

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
- Visibility minimization (MANDATORY axis) — any item (any current visibility) with no callers outside its defining module MUST be private. Manual grep-based check; the `unreachable_pub` lint does NOT catch this.
- Visibility minimization (OPTIONAL axis) — `pub` items with no cross-crate caller may be demoted to `pub(crate)`; library crates may keep `pub` for intentional API surface. The "container already narrow" exception above still applies. `#![warn(unreachable_pub)]` can be enabled temporarily to audit this axis but is not on by default.
- Item ordering — private (no-modifier) items declared after `pub` / exported ones (see "Item ordering — private items last")

If you add a tool or script that detects any of these, move the corresponding entry into the tool-enforced list above.

## Existing legitimate exceptions

- (None recorded yet — append entries here as they are discovered, with a brief justification.)
