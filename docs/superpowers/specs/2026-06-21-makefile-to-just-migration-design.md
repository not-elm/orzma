# Makefile → justfile Migration Design

**Date:** 2026-06-21
**Status:** Draft (pending spec review)

## Goal

Replace the repository-root `Makefile` with a `justfile` and delete the
`Makefile`, making [`just`](https://just.systems/) the sole task runner for the
ozmux polyglot monorepo (Rust/cargo + TypeScript/pnpm + Python scripts).

Recipe names are kept **identical** to the current Make targets (`just run`,
`just setup-cef`, `just fix-lint`, …) to minimize the change to muscle memory
and documentation.

## Motivation

A parallel-research investigation (Codex + web) concluded that `just` is the
strongest fit for this repo:

- It is the de-facto modern Make replacement (34k★, Rust), and maps the current
  Makefile's features (recipe dependencies, variables, one conditional, argument
  passthrough) almost 1:1 while dropping Make's tab/`.PHONY`/timestamp baggage.
- The closest real-world precedents are Rust+TypeScript monorepos that already
  orchestrate cargo + pnpm from a single `justfile`: **Biome** and **pnpm**
  itself.
- Heavier alternatives were rejected: `mise` (also manages toolchain versions,
  but broader scope than needed now), Turborepo/Nx (JS-build-graph caching, not
  a polyglot orchestrator), `cargo-make` (too Rust-centric), `mage` (Go
  dependency), `doit`/`mask` (poorer fit for a phony command runner).

The current Makefile is a thin phony command runner — every target shells out to
`cargo` / `pnpm` / `python3`. It uses Make-specific features only minimally:
one conditional (`$(if $(CARGO_HOME),...)`), two target dependencies
(`ozmd: ozmd-web`, `release-macos: setup-cef-release`), variables, and one
argument passthrough (`BUNDLE_ARGS`). No file-timestamp incremental build is
used. This makes the migration low-risk.

## Scope of affected files

`make` is referenced in 9 places across the repo (verified via grep):

| File | Reference | Action |
|---|---|---|
| `.github/workflows/release-macos.yml:33` | `make setup-cef-release` | Replace with `just setup-cef-release`; add a `just` install step |
| `README.md:29` | `make setup-cef` | → `just setup-cef`; add a `just` prerequisite |
| `README.md:36` | `make run` | → `just run` |
| `CLAUDE.md:26` | "pinned … in the Makefile"; `make setup-cef` | → "in the justfile"; `just setup-cef` |
| `CLAUDE.md:49,50,54,55` | command table (`make build/run/fix-lint/setup-cef`) | → `just …` |
| `.claude/rules/typescript.md:137` | `make fix-lint` | → `just fix-lint` |
| `.claude/rules/rust.md:516` | `make fix-lint` | → `just fix-lint` |
| `Cargo.toml:15` | comment `make run` | → `just run` |
| `scripts/bundle_macos.py:184` | error message `run \`make setup-cef-release\`` | → `just setup-cef-release` |

Neither `just` nor `mise` is currently installed on the dev machine or in CI, so
the design must include install instructions (docs) and a CI install step.

## Design

### 1. The `justfile`

Created at the repository root. Variable mapping from the current Makefile:

| Makefile | justfile |
|---|---|
| `CEF_VERSION := 145.6.1+145.0.28` | `cef_version := "145.6.1+145.0.28"` |
| `$(HOME)/.local/share/cef` | `cef_dir := home_directory() / ".local/share/cef"` |
| `CEF_FRAMEWORK_LIB` | `cef_framework_lib := cef_dir / "Chromium Embedded Framework.framework" / "Libraries"` |
| `CARGO_BIN_DIR := $(if $(CARGO_HOME),$(CARGO_HOME)/bin,$(HOME)/.cargo/bin)` | `cargo_bin_dir := if env("CARGO_HOME", "") == "" { home_directory() / ".cargo" / "bin" } else { env("CARGO_HOME", "") / "bin" }` |
| `CEF_DEBUG_RENDER_PROCESS` | `cef_debug_render_process := "bevy_cef_debug_render_process"` |
| `BEVY_CEF_RENDER_PROCESS` | `bevy_cef_render_process := "bevy_cef_render_process"` |
| `BEVY_CEF_GIT` | `bevy_cef_git := "https://github.com/not-elm/bevy_cef"` |
| `BEVY_CEF_BRANCH` | `bevy_cef_branch := "passthrough"` |

just's `env(name, default)` returns `default` only when the variable is
**absent**, not when it is set-but-empty (it then returns the empty string).
Make's `$(if $(CARGO_HOME),…)` treats a set-but-empty value as false. To match
Make exactly, the port uses a just `if/else` expression: when `CARGO_HOME` is
unset or empty it falls back to `$HOME/.cargo/bin`, otherwise it uses
`$CARGO_HOME/bin`.

Recipe mapping (the 10 command targets; the `help` target is handled by
improvement #1 below):

| Make target | just recipe |
|---|---|
| `run` | `cargo run` |
| `build` | `cargo build` |
| `clean` | `cargo clean` |
| `setup-cef` | 4 lines: `cargo install export-cef-dir@{{cef_version}} --force`; `export-cef-dir --force "{{cef_dir}}"`; `cargo install {{cef_debug_render_process}}`; `cp "{{cargo_bin_dir}}/…" "{{cef_framework_lib}}/…"` |
| `fix-lint` | 3 lines: `cargo clippy --workspace --fix --allow-dirty --allow-staged`; `cargo fmt`; `pnpm lint:fix` |
| `ozmd-web` | `pnpm --filter @ozma/ozmd-web build` |
| `ozmd` | depends on `ozmd-web`, then `cargo build -p ozmd` |
| `setup-cef-release` | 3 lines: export-cef-dir install + run + `cargo install --git {{bevy_cef_git}} --branch {{bevy_cef_branch}} {{bevy_cef_render_process}}` |
| `bundle-macos *args` | `python3 scripts/bundle_macos.py {{args}}` |
| `release-macos *args` | depends on `setup-cef-release`, then `python3 scripts/bundle_macos.py --notarize {{args}}` |

`BUNDLE_ARGS` is replaced by just's variadic parameter `*args`, which collects
trailing arguments into a space-joined string (e.g.
`just bundle-macos --version 1.2.3`).

Multi-line recipes (`setup-cef`, `fix-lint`, `setup-cef-release`) run each line
in its own shell, exactly as Make does; just stops on the first failing line.
Paths containing spaces (the CEF framework directory) are quoted, as in the
current Makefile.

### 2. Idiomatic improvements (recommended)

Three small, justified improvements over a literal 1:1 port:

1. **Replace the hand-written `help` body with `just --list`, keeping the
   `help` name and making it the default.** Running `just` with no argument, or
   `just help`, lists all recipes:
   ```just
   # list all recipes (also the default when run with no arguments)
   default: help

   # show available recipes
   help:
       @just --list
   ```
   This preserves `just help` (honoring the identical-name goal) while removing
   the manually maintained echo block, which can drift out of sync with the
   real targets.

2. **Drop the unused `OZMUX_EXTENSION_ROOT` variable.** It is defined in the
   Makefile (`$(CURDIR)/extensions`) but referenced by no target. YAGNI.

3. **Annotate macOS-only recipes with `[macos]`.** `setup-cef`,
   `setup-cef-release`, `bundle-macos`, and `release-macos` are macOS-specific
   (CEF framework layout, `codesign`/`notarytool`). The `[macos]` attribute makes
   `just` error clearly on other platforms instead of failing midway — a guard
   the current Makefile lacks.

### 3. CI change

`.github/workflows/release-macos.yml` is the only CI file that invokes `make`:

- Add a `just` install step before the "Provision CEF" step. Recommended:
  `taiki-e/install-action@just` (installs a prebuilt binary, integrates with the
  existing Rust tooling cache; alternative: `extractions/setup-just@v4`).
- Change line 33 `run: make setup-cef-release` → `run: just setup-cef-release`.

`ci-rust.yml` and `ci-ts.yml` call `cargo` / `pnpm` directly and never used
`make`, so they are unchanged.

### 4. Documentation & comment updates

Mechanically update the `make` references listed in **Scope of affected files**:

- `README.md`: `make setup-cef` → `just setup-cef`, `make run` → `just run`, and
  add a Prerequisites bullet: install `just` (`cargo install just` or
  `brew install just`).
- `CLAUDE.md`: command table and prose (`make …` → `just …`; "in the Makefile" →
  "in the justfile").
- `.claude/rules/typescript.md`, `.claude/rules/rust.md`: `make fix-lint` →
  `just fix-lint`.
- `Cargo.toml:15` comment: `make run` → `just run`.
- `scripts/bundle_macos.py:184`: user-facing error message
  ``run `make setup-cef-release` `` → ``run `just setup-cef-release` ``.

### 5. Delete the Makefile

After the `justfile`, CI, and docs are updated, delete the root `Makefile`. The
`justfile` becomes the single source of truth.

## Verification

This is a configuration migration; there are no unit tests. Verification:

- `just --fmt --check` — formatting is canonical (on older `just` versions
  `--fmt` requires `--unstable`; the install step should pull a recent `just`).
- `just --list` — all recipes are present and named identically to the old
  targets.
- `just --dry-run <recipe>` / `just --evaluate` — confirm command/variable
  expansion is correct for the heavy or side-effecting recipes (`setup-cef`,
  `setup-cef-release`, `release-macos`) **without** running network installs or
  notarization.
- Run the light recipes (`build`, `run`, `fix-lint`, `ozmd`) once and confirm
  behavior matches the old Make targets.
- The `release-macos` workflow's `just setup-cef-release` exercises the release
  path in CI.

## Out of scope (YAGNI)

- Toolchain-version management integration (`mise`).
- Restructuring pnpm scripts (the TypeScript side stays as-is; `just` recipes
  shell out to `pnpm`).
- Build-graph caching tools (Turborepo / Nx).
