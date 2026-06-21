# Rust Coding Rules

Rust code in this repo (`src/`, `crates/*`) follows the rules below. Edition 2024, toolchain pinned to 1.95. These rules complement the conventions documented in `CLAUDE.md`.

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

Required — import, don't inline:

- Prefer `use` imports over inline fully-qualified paths. If a type is used in a function signature or system body, it belongs in the `use` block at the top of the file, not written out inline as `some_crate::some_module::Type` at the call site.

| Pattern | Example | Fix |
| --- | --- | --- |
| Inline path in signature / body | `fn f(x: foo::bar::Baz)` | Add `use foo::bar::Baz;` and write `fn f(x: Baz)` |
| Inline path in `run_if` or type parameter | `.add_message::<bevy::window::WindowResized>()` | `use bevy::window::WindowResized;` then `.add_message::<WindowResized>()` |

## Naming — Query parameters

Bevy `Query` system parameters must not use a `_q` suffix. Use a descriptive noun instead:

- Singular (`window`, `terminal`) when the query is expected to return one result and the system calls `.single()` / `.single_mut()`.
- Plural (`windows`, `terminals`) when the query is iterated or used with `.get()` over an arbitrary entity.

Forbidden:

| Pattern | Example | Fix |
| --- | --- | --- |
| `_q` suffix on any `Query` parameter | `window_q: Query<&Window, …>` | `window: Query<&Window, …>` |

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

## Parameter ordering — mutable parameters first

Within a function or method signature, declare **mutable** parameters
before **immutable** ones. A parameter is mutable when its binding is
`mut` or it carries mutable access — e.g. `mut s: String`,
`buf: &mut Vec<u8>`, `mut commands: Commands`,
`mut windows: Query<&mut Window>`, `mut config: ResMut<Config>`. A
parameter is immutable when it is a non-`mut` by-value or shared-access
binding — e.g. `name: &str`, `windows: Query<&Window>`,
`config: Res<Config>`, `keys: Res<ButtonInput<KeyCode>>`.

Rationale: grouping the parameters a function writes through ahead of
the ones it only reads makes the call's effect surface visible at the
signature — the same "surface first" reasoning behind item ordering.

Required:

| Pattern | Example |
| --- | --- |
| Mutable params grouped first, then immutable | `fn reflow(mut windows: Query<&mut Window>, settings: Res<Settings>)` |

Forbidden:

| Pattern | Example | Why |
| --- | --- | --- |
| An immutable param ahead of a mutable one | `fn reflow(settings: Res<Settings>, mut windows: Query<&mut Window>)` | Mutable params must come first |

Exceptions — these override the style rule:

- A **fixed structural leading position** is exempt and never reordered: a
  method's `self` receiver (`&self` / `&mut self`), and a Bevy observer
  system's `On<E>` trigger (the system *input*, which `bevy_ecs` requires to
  be first). The mutable-first ordering governs only the parameters that
  **follow** such a slot — e.g.
  `fn on_paste(ev: On<E>, mut clipboard: ResMut<Clipboard>, q: Query<&T>)` is
  compliant: `ev` is fixed first, and the params after it are mutable-first.
- A **semantic ordering requirement** wins. When parameter order carries
  meaning — e.g. Bevy `SystemParam`s with separate deferred command
  queues that must apply in a specific order (a `mux` param before a
  `commands` param so entity spawns flush before the components inserted
  on them) — order for correctness and record why in a `// NOTE:`.
- Trait-method `impl`s whose signature is dictated by the trait.
- `#[cfg(test)] mod tests { ... }` contents are exempt.

## System optimization — gate with `run_if`, not in-body change checks

A Bevy system that begins by checking a resource's change state and
returning early — `if !res.is_changed() { return; }` (or
`if !res.is_added() { return; }`) — must instead be gated at
registration with a run condition. The early-return form still pays the
cost of scheduling the system, acquiring its `SystemParam` data access,
and running its body up to the guard every frame; a `run_if` condition
skips the system entirely when the condition is false, and lets the
scheduler reason about the dependency.

Required:

| Instead of (in-body) | Use (at registration) |
| --- | --- |
| `fn sys(res: Res<T>) { if !res.is_changed() { return; } ... }` | `sys.run_if(resource_exists_and_changed::<T>)` |
| `fn sys(res: Res<T>) { if !res.is_added() { return; } ... }` | `sys.run_if(resource_added::<T>)` |

- Prefer `resource_exists_and_changed::<T>` over bare
  `resource_changed::<T>` unless the resource is guaranteed to exist for
  the system's whole lifetime; the `_exists_` variant will not panic if
  the resource is absent.
- After moving the guard into a `run_if`, delete the in-body early
  return — leaving both is redundant and misleads the reader about when
  the system runs. Note the gating in the system's doc comment instead.
- Keep any test that registers the same system in sync: add the matching
  `run_if` so the test exercises the real scheduling behavior.

Not covered by this rule (leave as-is):

- Per-entity / per-component change detection inside a query loop
  (`query.iter().any(|c| c.is_changed())`) — that is not a whole-system
  gate and has no `run_if` equivalent.
- Bodies that branch on change state to do *different* work (not an
  all-or-nothing early return).

## Change detection — let mutation drive it, don't force it manually

Bevy emits a change notification automatically when you write through a
`ResMut` / `Mut` (any `DerefMut`); readers gate on that via `run_if`
(see the section above) or `Changed<T>` / `Added<T>` queries. A design
that follows ordinary ECS data flow therefore never needs to *manually*
announce that something changed. Manual notification breaks the contract
that "changed" means "the value actually changed", so every downstream
`run_if`/`Changed` consumer can no longer trust it.

Forbidden:

| Pattern | Why |
| --- | --- |
| `res.set_changed()` / `query_item.set_changed()` | Forces a notification the mutation itself should have produced |
| `*res.bypass_change_detection() = …; res.set_changed();` | Suppresses the real change then re-emits it by hand — the honest form is one ordinary write through `&mut` |
| `res.bypass_change_detection()` used to *hide* a genuine mutation from readers | Silently desyncs consumers gated on `Changed` / `run_if` |

Root cause and fix: this dance almost always appears because the code
writes through the mutable reference **unconditionally every frame** (so
naive change detection would fire every frame) and then tries to undo
that with `bypass_change_detection()` + a conditional `set_changed()`.
The ECS-aligned fix is to mutate **conditionally** — compute the next
value from an immutable read, compare, and write through the normal
`&mut` only when it differs. Change detection then fires exactly on real
changes, for free:

```rust
// Avoid: unconditional deref_mut, then hand-managed notification.
let changed = step(state.bypass_change_detection(), &events);
if changed { state.set_changed(); }

// Prefer: write through ResMut only on a real change.
let mut next = state.clone();
if step(&mut next, &events) {
    *state = next; // the single DerefMut — change fires here, only when it differs
}
```

The same applies to components: don't assign an identical value every
frame; guard the write with an equality check (the renderer's "only write
the `Node` fields when they actually change" pattern is the model).

Escape hatch (justify with a `// NOTE:`, and `#[expect]` where a lint
applies): genuine interior mutation that change detection cannot observe
— e.g. a component owning a handle/buffer whose *contents* are mutated in
place (no `DerefMut` on the component) while a downstream system must
still be told — or a documented workaround for a specific upstream Bevy
bug. "It's simpler" or "I mutate it every frame anyway" is not a valid
reason; mutate conditionally instead.

## Bevy `Plugin::build` — method chaining

All `App` configuration calls inside a `Plugin::build` body must be written
as a single method-chain off the first call rather than as repeated `app.`
statements. This keeps the registration block visually unified and avoids
redundant `app.` noise.

Required:

```rust
// Correct: one chain, semicolon only at the end.
fn build(&self, app: &mut App) {
    app.init_resource::<Foo>()
        .add_systems(Update, my_system)
        .add_observer(my_observer)
        .add_plugins(SubPlugin);
}
```

Forbidden:

```rust
// Wrong: each call re-states `app.`.
fn build(&self, app: &mut App) {
    app.init_resource::<Foo>();
    app.add_systems(Update, my_system);
    app.add_observer(my_observer);
}
```

Exception: a call that must be preceded by local logic (e.g., a conditional
`if` that decides whether to register a system) may start a new `app.` chain
for the calls after the branch. Keep each such sub-chain as long as possible;
do not split further than the branch requires.

## Plugin registration — register in the defining file's plugin

Systems and observers are registered by a `Plugin` defined in the SAME file
that defines them, not by `add_systems` / `add_observer` in an upstream /
aggregator plugin in another file. An aggregator plugin composes the per-file
plugins with `add_plugins`.

Rationale: a file's ECS registration stays self-contained and discoverable next
to the systems it registers; parent plugins remain thin aggregators.

Required:

- A file that defines systems/observers also defines the `Plugin` that registers
  them; parent modules include it via `add_plugins`.

Forbidden:

| Pattern | Why |
| --- | --- |
| `add_systems` / `add_observer` in a parent/aggregator plugin for a system or observer defined in a different file | Hoists registration away from the code it registers |

Exception:

- Cross-file ordering coupling (system A in file X must run before system B in
  file Y) is expressed with `.after()` / `.before()` / a shared `SystemSet`
  across the per-file plugins — NOT by hoisting both registrations into one
  upstream plugin.

## System composition — keep systems focused; split by responsibility

A Bevy system that, in one body, **gathers** input, **decides** what to do, and
**applies** the result is doing three jobs at once: it grows long, mixes
immutable reads with broad `&mut` access, and traps the decision logic behind
ECS params (and resources with no public constructor), making it untestable.
Split such systems along the gather → decide → apply seam.

- **Pure decision helpers** take plain data and return *effect values* (an enum
  / `Vec` of intents), touching no world state — unit-testable without an `App`.
  Example: `decide_button` / `decide_wheel` (`crates/ozma_terminal/src/mouse.rs`)
  return `Vec<MouseEffect>`.
- **Apply via an `EntityEvent` + observer** (the repo idiom). The gather system
  queries the target **immutably**, computes effects, and `commands.trigger(...)`s
  them; the observer holds the `&mut` access and writes the world. See
  `dispatch_input` → `TerminalKeyInput` → `on_terminal_key_input`
  (`crates/ozma_tty_engine/src/plugin.rs`) and `PasteAction` / `on_paste`
  (`crates/ozma_terminal/src/action.rs`).
- **Extract bulky inline blocks** into named helper `fn`s so the body reads as
  gate → collect → trigger. Gate preconditions with `run_if` (see "System
  optimization — gate with `run_if`").

Required:

| Instead of (one system does everything) | Use |
| --- | --- |
| Read input, decide, and write the world inline in one system | A pure decide helper returning effect values + an `EntityEvent`/observer that applies them |
| Decision logic interleaved with `&mut` world access | Decision over borrowed data returning intents; mutation isolated to the apply observer |
| A 40-plus-line inline block in a system body | A named helper `fn` the system calls |

Forbidden:

| Pattern | Why |
| --- | --- |
| `.pipe()` to chain a gather system into an apply system | Not this repo's idiom — it sequences with `EntityEvent` + observer and `.chain()` / system-sets, and uses `.pipe()` nowhere |
| A long system whose only structure is sequential gather/decide/apply phases that could each be a helper or observer | Defeats single-responsibility; hides the apply surface |

Rationale: a gather system that ends in `commands.trigger(Effects { .. })` makes
its effect on the world legible at the signature, and the observer is the one
place to look for mutation. The pure decider is testable without a PTY / GPU /
`App`, and an immutable gather query plus a `&mut` apply observer never contend
for the same data (separate systems; the observer runs at command flush).

Exceptions:

- A genuinely **single-purpose** system (only gathers, only decides, or only
  applies) is already focused — leave it.
- **Do not over-fragment.** Splitting into several systems that each re-query
  the same components (duplicate access / scheduling cost) is worse than the
  monolith when a private helper `fn` would do. Prefer a helper `fn` unless the
  apply step needs isolated `&mut` / NonSend access (the observer case) or
  independent scheduling.
- Some apply steps cannot be made pure (NonSend resources, async round-trip
  state). Move what you can to the observer; keep the irreducible reads in the
  gather system and record why in a `// NOTE:`.

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
- Parameter ordering — mutable parameters declared before immutable ones in function signatures (see "Parameter ordering — mutable parameters first")
- System optimization — whole-system resource change/added guards expressed as in-body early returns must be `run_if` run conditions instead (see "System optimization — gate with `run_if`, not in-body change checks")
- Change detection — no manual `set_changed()` / `bypass_change_detection()`-then-`set_changed()` notification; mutate conditionally so normal `DerefMut` drives change detection (see "Change detection — let mutation drive it, don't force it manually")
- Imports — no inline fully-qualified paths in signatures, bodies, or type parameters; add a `use` at the top instead (see "Imports — import, don't inline")
- Naming — `Query` parameters must not use a `_q` suffix; use a descriptive singular or plural noun (see "Naming — Query parameters")
- System composition — long systems that interleave gather/decide/apply must be split: pure decision helpers returning effect values, apply via an `EntityEvent`+observer (or a focused apply system), bulky inline blocks extracted to helpers (see "System composition — keep systems focused; split by responsibility")

If you add a tool or script that detects any of these, move the corresponding entry into the tool-enforced list above.

## Existing legitimate exceptions

- `frame_builder.rs` builder signatures keep `interner: &mut HyperlinkInterner` as the LAST parameter despite the mutable-params-first rule: the interner is an output-cache argument by convention across ~17 call sites, and reordering would churn them all for no readability gain.
