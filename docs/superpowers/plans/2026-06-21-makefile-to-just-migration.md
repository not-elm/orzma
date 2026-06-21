# Makefile → justfile Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the repository-root `Makefile` with a `justfile`, make [`just`](https://just.systems/) the sole task runner, and update every `make` reference across CI and docs.

**Architecture:** The current Makefile is a thin phony command runner that shells out to `cargo` / `pnpm` / `python3`. We port each target 1:1 to a `just` recipe (identical names), express the one Make conditional with a just `if/else`, guard the four macOS-only recipes with `[macos]`, replace the hand-written `help` with a `--list`-backed recipe, switch the single CI reference and all docs, then delete the Makefile last so the repo stays working at every commit.

**Tech Stack:** `just` (Rust task runner), GitHub Actions (`taiki-e/install-action`), Markdown docs.

## Global Constraints

- `just` is the **sole** task runner after this work; the `Makefile` is deleted.
- Recipe names are kept **identical** to the former Make targets (`run`, `build`, `clean`, `fix-lint`, `ozmd-web`, `ozmd`, `setup-cef`, `setup-cef-release`, `bundle-macos`, `release-macos`, `help`).
- macOS-only recipes (`setup-cef`, `setup-cef-release`, `bundle-macos`, `release-macos`) carry the `[macos]` attribute.
- CEF version pinned to `145.6.1+145.0.28`; bevy_cef git `https://github.com/not-elm/bevy_cef`, branch `passthrough`.
- All in-file comments (incl. the `justfile`) must be **English** (CLAUDE.md rule).
- `.github/workflows/release-macos.yml` is the **only** CI workflow that references `make`; `ci-rust.yml` / `ci-ts.yml` call `cargo`/`pnpm` directly and are not touched.
- Work happens on branch `tool` (not `main`); per-task commits are expected.
- The repo must build/run at every commit — add the `justfile` first, switch references, delete the `Makefile` last.

---

### Task 1: Create and verify the `justfile`

**Files:**
- Create: `justfile` (repo root)
- Reference (do NOT edit yet): `Makefile`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: a `justfile` at the repo root exposing recipes `default`, `help`, `run`, `build`, `clean`, `fix-lint`, `ozmd-web`, `ozmd`, `setup-cef`, `setup-cef-release`, `bundle-macos *args`, `release-macos *args`. Later tasks rely on the recipe names `setup-cef-release` (CI) and the existence of the file (delete-Makefile task).

- [ ] **Step 1: Install `just` locally (one-time, not committed)**

Run (macOS):
```bash
brew install just || cargo install just
just --version
```
Expected: prints a version (e.g. `just 1.x.y`). `just` is not currently installed, so this must succeed before the file can be verified.

- [ ] **Step 2: Create the `justfile`**

Create `justfile` at the repo root with exactly this content:

```just
# ozmux task runner. Ports the former Makefile; see
# docs/superpowers/specs/2026-06-21-makefile-to-just-migration-design.md.
# https://just.systems/

cef_version := "145.6.1+145.0.28"
cef_dir := home_directory() / ".local/share/cef"
cef_framework_lib := cef_dir / "Chromium Embedded Framework.framework" / "Libraries"
cef_debug_render_process := "bevy_cef_debug_render_process"
bevy_cef_render_process := "bevy_cef_render_process"
bevy_cef_git := "https://github.com/not-elm/bevy_cef"
bevy_cef_branch := "passthrough"

# CARGO_HOME/bin when CARGO_HOME is set and non-empty, else ~/.cargo/bin.
# env(key, default) returns the default only when the var is ABSENT, so the
# set-but-empty case is handled explicitly to match Make's $(if ...).
cargo_bin_dir := if env("CARGO_HOME", "") == "" { home_directory() / ".cargo" / "bin" } else { env("CARGO_HOME", "") / "bin" }

# list all recipes (also the default when run with no arguments)
default: help

# show available recipes
help:
    @just --list

# run the ozmux Bevy app
run:
    cargo run

# build the workspace
build:
    cargo build

# remove the workspace target dir
clean:
    cargo clean

# clippy --fix + rustfmt + biome lint:fix
fix-lint:
    cargo clippy --workspace --fix --allow-dirty --allow-staged
    cargo fmt
    pnpm lint:fix

# build the ozmd web bundle (esbuild)
ozmd-web:
    pnpm --filter @ozma/ozmd-web build

# build the web bundle then the ozmd binary
ozmd: ozmd-web
    cargo build -p ozmd

# install the CEF framework + debug render process (macOS, one-time)
[macos]
setup-cef:
    cargo install export-cef-dir@{{cef_version}} --force
    export-cef-dir --force "{{cef_dir}}"
    cargo install {{cef_debug_render_process}}
    cp "{{cargo_bin_dir}}/{{cef_debug_render_process}}" "{{cef_framework_lib}}/{{cef_debug_render_process}}"

# install arm64 CEF + release render process (for bundling)
[macos]
setup-cef-release:
    cargo install export-cef-dir@{{cef_version}} --force
    export-cef-dir --force "{{cef_dir}}"
    cargo install --git {{bevy_cef_git}} --branch {{bevy_cef_branch}} {{bevy_cef_render_process}}

# build and package the ozmux .app (extra args pass through, e.g. --version 1.2.3)
[macos]
bundle-macos *args:
    python3 scripts/bundle_macos.py {{args}}

# setup-cef-release then bundle with notarization
[macos]
release-macos *args: setup-cef-release
    python3 scripts/bundle_macos.py --notarize {{args}}
```

- [ ] **Step 3: Verify the file parses and lists all recipes**

Run: `just --list`
Expected: lists every recipe with its description — `default`, `help`, `run`, `build`, `clean`, `fix-lint`, `ozmd-web`, `ozmd`, `setup-cef`, `setup-cef-release`, `bundle-macos`, `release-macos`. No parse error.

- [ ] **Step 4: Verify variable expansion (the `if/else` for `cargo_bin_dir`)**

Run: `just --evaluate`
Expected: prints variable values. `cargo_bin_dir` resolves to `<home>/.cargo/bin` (when `CARGO_HOME` is unset), and `cef_framework_lib` ends with `.../Chromium Embedded Framework.framework/Libraries`. No `/bin` (leading-slash) result.

Then confirm the set-but-empty edge case matches Make (falls back to home, not `/bin`):
Run: `CARGO_HOME= just --evaluate | grep cargo_bin_dir`
Expected: still `<home>/.cargo/bin` (NOT `/bin`).

- [ ] **Step 5: Verify the macOS recipes expand correctly without running (dry-run)**

Run: `just -n setup-cef`
Expected: prints the 4 commands with variables expanded (e.g. `cargo install export-cef-dir@145.6.1+145.0.28 --force`, the quoted `cp` line) — nothing is executed.

Run: `just -n release-macos --version 1.2.3`
Expected: prints the `setup-cef-release` dependency commands first, then `python3 scripts/bundle_macos.py --notarize --version 1.2.3` — confirming both the dependency chain and the `*args` passthrough.

- [ ] **Step 6: Verify formatting is canonical**

Run: `just --unstable --fmt --check`
Expected: exit 0 (no diff). If it reports the file would be reformatted, run `just --unstable --fmt` to apply the canonical formatting, then re-run `just --list` (Step 3) to confirm it still parses, and keep the formatted file.

- [ ] **Step 7: Commit**

```bash
git add justfile
git commit -m "build: add justfile porting the Makefile targets

just becomes the task runner; recipe names mirror the former Make targets.
The Makefile is still present and is removed in a later commit.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Switch the release CI workflow from `make` to `just`

**Files:**
- Modify: `.github/workflows/release-macos.yml:32-33`

**Interfaces:**
- Consumes: the `setup-cef-release` recipe from Task 1.
- Produces: a CI workflow that installs `just` and runs `just setup-cef-release`.

- [ ] **Step 1: Add a `just` install step and switch the run command**

In `.github/workflows/release-macos.yml`, replace this block:

```yaml
      - name: Provision CEF + release render process
        run: make setup-cef-release
```

with:

```yaml
      - name: Install just
        uses: taiki-e/install-action@just

      - name: Provision CEF + release render process
        run: just setup-cef-release
```

- [ ] **Step 2: Verify the workflow is still valid YAML and `make` is gone**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release-macos.yml')); print('ok')"`
Expected: `ok` (valid YAML).

Run: `grep -n 'make ' .github/workflows/release-macos.yml || echo "no make references"`
Expected: `no make references`.

Run: `grep -n 'just setup-cef-release\|taiki-e/install-action@just' .github/workflows/release-macos.yml`
Expected: both lines present.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release-macos.yml
git commit -m "ci(release-macos): install just and run just setup-cef-release

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Update docs, comments, and the bundler error message

**Files:**
- Modify: `README.md` (Prerequisites + Run)
- Modify: `CLAUDE.md` (prose at the CEF line + the Commands table)
- Modify: `.claude/rules/typescript.md:137`
- Modify: `.claude/rules/rust.md:516`
- Modify: `Cargo.toml:15`
- Modify: `scripts/bundle_macos.py:184`

**Interfaces:**
- Consumes: recipe names from Task 1.
- Produces: zero remaining `make` references in tracked files (verified in Step 7).

- [ ] **Step 1: Update `README.md` — add a `just` prerequisite and switch `make` → `just`**

In `README.md`, replace:

```markdown
- Node + `pnpm@10.30.2` (for the `@ozma/web` TypeScript package; dev/CI use Node 24)
- The Chromium Embedded Framework, installed once:
  ```bash
  make setup-cef
  ```
```

with:

```markdown
- Node + `pnpm@10.30.2` (for the `@ozma/web` TypeScript package; dev/CI use Node 24)
- [`just`](https://just.systems/) — the task runner (`brew install just` or `cargo install just`)
- The Chromium Embedded Framework, installed once:
  ```bash
  just setup-cef
  ```
```

Then replace:

```markdown
cargo run               # or: make run
```

with:

```markdown
cargo run               # or: just run
```

- [ ] **Step 2: Update `CLAUDE.md` — prose**

Replace `pinned to `145.6.1+145.0.28` in the Makefile)` with `pinned to `145.6.1+145.0.28` in the justfile)`.

Replace `see `make setup-cef`.` with `see `just setup-cef`.`.

- [ ] **Step 3: Update `CLAUDE.md` — the Commands table**

Replace `cargo build` (or `make build`)` with `cargo build` (or `just build`)`.

Replace `cargo run` (or `make run`)` with `cargo run` (or `just run`)`.

Replace `make fix-lint` (runs clippy fix` with `just fix-lint` (runs clippy fix`.

Replace `make setup-cef` (installs the CEF framework` with `just setup-cef` (installs the CEF framework`.

- [ ] **Step 4: Update the rule files**

In `.claude/rules/typescript.md`, replace:
```markdown
- Run via `pnpm lint` / `pnpm lint:fix` / `make fix-lint`
```
with:
```markdown
- Run via `pnpm lint` / `pnpm lint:fix` / `just fix-lint`
```

In `.claude/rules/rust.md`, replace:
```markdown
- `cargo clippy --fix --allow-dirty --allow-staged && cargo fmt`, or `make fix-lint`
```
with:
```markdown
- `cargo clippy --fix --allow-dirty --allow-staged && cargo fmt`, or `just fix-lint`
```

- [ ] **Step 5: Update the `Cargo.toml` comment**

In `Cargo.toml`, replace:
```toml
# Default-on for development: `cargo run` / `make run` load CEF from
```
with:
```toml
# Default-on for development: `cargo run` / `just run` load CEF from
```

- [ ] **Step 6: Update the bundler error message**

In `scripts/bundle_macos.py`, replace:
```python
            f"CEF framework not found: {cfg.cef_framework} (run `make setup-cef-release`)"
```
with:
```python
            f"CEF framework not found: {cfg.cef_framework} (run `just setup-cef-release`)"
```

- [ ] **Step 7: Verify no `make` references remain (Makefile itself still present)**

Run:
```bash
grep -rn --include='*.md' --include='*.yml' --include='*.yaml' --include='*.toml' --include='*.json' --include='*.py' -E '\bmake [a-z]' . | grep -v node_modules | grep -v target | grep -vi 'make the\|make sure\|make scenario\|makes\|make it\|make a '
```
Expected: no output (every `make <target>` reference is now `just <recipe>`). The only acceptable matches are English prose like "make sure" / "makes" in the rule files — confirm none of the remaining lines is a `make <recipe>` invocation.

- [ ] **Step 8: Commit**

```bash
git add README.md CLAUDE.md .claude/rules/typescript.md .claude/rules/rust.md Cargo.toml scripts/bundle_macos.py
git commit -m "docs: point make references at just

README/CLAUDE/rules/Cargo comment and the bundler error message now refer to
just recipes instead of make targets.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Delete the `Makefile` and do a final verification

**Files:**
- Delete: `Makefile`

**Interfaces:**
- Consumes: a working `justfile` (Task 1), switched CI (Task 2), switched docs (Task 3).
- Produces: a repo with `just` as the sole task runner.

- [ ] **Step 1: Confirm the justfile fully covers the Makefile targets**

Run:
```bash
grep -E '^[a-z][a-z-]*:' Makefile | sed 's/:.*//' | sort -u
just --summary | tr ' ' '\n' | sort -u
```
Expected: every Makefile target name (`run`, `build`, `clean`, `help`, `fix-lint`, `ozmd-web`, `ozmd`, `setup-cef`, `setup-cef-release`, `bundle-macos`, `release-macos`) appears in the `just --summary` output. (`just --summary` may need `--unstable` on older versions: `just --unstable --summary`.)

- [ ] **Step 2: Delete the Makefile**

Run: `git rm Makefile`
Expected: `rm 'Makefile'`.

- [ ] **Step 3: Final verification — repo no longer depends on make**

Run: `just --list`
Expected: still lists all recipes (justfile is intact).

Run: `test -f Makefile && echo "STILL PRESENT" || echo "deleted"`
Expected: `deleted`.

Run:
```bash
grep -rn 'make setup-cef\|make run\|make build\|make fix-lint' . | grep -v node_modules | grep -v target | grep -v docs/superpowers || echo "no make-target references outside the design/plan docs"
```
Expected: `no make-target references outside the design/plan docs` (the spec/plan under `docs/superpowers/` legitimately quote the old `make` commands as history).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "build: remove the Makefile; just is the sole task runner

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- `just` is NOT preinstalled; Task 1 Step 1 installs it. CI installs it via `taiki-e/install-action@just` (Task 2).
- Do not run the heavy recipes (`setup-cef`, `setup-cef-release`, `release-macos`, `run`) for real during verification — they download/compile/launch the GUI. Dry-run (`just -n <recipe>`) and `just --evaluate` are the gates.
- The `[macos]` attribute means the four guarded recipes only exist on macOS; on Linux/Windows `just <that-recipe>` errors with "Justfile does not contain recipe", which is the intended guard. We verify on macOS only.
- Keep every commit in a working state: justfile added first, references switched, Makefile deleted last.
