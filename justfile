# orzma task runner. Ports the former Makefile; see
# docs/superpowers/specs/2026-06-21-makefile-to-just-migration-design.md.
# https://just.systems/

cef_version := "145.6.1+145.0.28"
cef_dir := home_directory() / ".local/share/cef"
cef_framework_lib := cef_dir / "Chromium Embedded Framework.framework" / "Libraries"
cef_debug_render_process := "bevy_cef_debug_render_process"
bevy_cef_render_process := "bevy_cef_render_process"
bevy_cef_version := "0.11.0"
cargo_about_version := "0.9.0"
pnpm_licenses_version := "2.4.2"

# CARGO_HOME/bin when CARGO_HOME is set and non-empty, else ~/.cargo/bin.
# env(key, default) returns the default only when the var is ABSENT, so the
# set-but-empty case is handled explicitly to match Make's $(if ...).
cargo_bin_dir := if env("CARGO_HOME", "") == "" { home_directory() / ".cargo" / "bin" } else { env("CARGO_HOME", "") / "bin" }

# list all recipes (also the default when run with no arguments)
default: help

# show available recipes
help:
    @just --list

# bump all package versions to <version> (updates VERSION, Cargo.toml, sdk/orzma-web/package.json)
bump-version version:
    bash scripts/bump-version.sh {{ version }}

# run the orzma Bevy app
run:
    cargo run

# build the workspace
build:
    cargo build

install-apps:
    pnpm i
    pnpm build
    cargo install --path ./apps/orzbrowser/
    cargo install --path ./apps/orzmd/

# remove the workspace target dir
clean:
    cargo clean

# clippy --fix + rustfmt + biome lint:fix
fix-lint:
    cargo clippy --workspace --fix --allow-dirty --allow-staged
    cargo fmt
    pnpm lint:fix

# build the orzmd web bundle (esbuild)
orzmd-web:
    pnpm --filter @orzma/orzmd-web build

# build the web bundle then the orzmd binary
orzmd: orzmd-web
    cargo build -p orzmd

# install the CEF framework + debug render process (macOS, one-time)
[macos]
setup-cef:
    cargo install export-cef-dir@{{ cef_version }} --force
    export-cef-dir --force "{{ cef_dir }}"
    cargo install {{ cef_debug_render_process }}@{{ bevy_cef_version }}
    cp "{{ cargo_bin_dir }}/{{ cef_debug_render_process }}" "{{ cef_framework_lib }}/{{ cef_debug_render_process }}"

# install arm64 CEF + release render process (for bundling)
[macos]
setup-cef-release:
    cargo install export-cef-dir@{{ cef_version }} --force
    export-cef-dir --force "{{ cef_dir }}"
    cargo install {{ bevy_cef_render_process }}@{{ bevy_cef_version }}

# regenerate the macOS app icon (build/macos/AppIcon.icns) from the master SVG
[macos]
icon *args:
    python3 scripts/build_icon.py {{ args }}

# build and package the orzma .app (extra args pass through, e.g. --version 1.2.3)
[macos]
bundle-macos *args: orzmd-web
    pnpm i
    pnpm build
    python3 scripts/bundle_macos.py {{ args }}

# setup-cef-release then bundle with notarization
[macos]
release-macos *args: setup-cef-release orzmd-web
    python3 scripts/bundle_macos.py --notarize {{ args }}

# refresh the vendored Chromium credits from the provisioned CEF dir (run on cef_version bump)
licenses-refresh-cef:
    cp "{{ cef_dir }}/CREDITS.html" licenses/chromium/CREDITS.html

# generate licenses/THIRD-PARTY-LICENSES.md from all dependency licenses
licenses:
    python3 scripts/generate_licenses.py --cargo-about-version {{ cargo_about_version }} --pnpm-licenses-version {{ pnpm_licenses_version }}

# regenerate and fail if the committed licenses file is stale (CI drift gate)
licenses-check: licenses
    git diff --exit-code licenses/THIRD-PARTY-LICENSES.md
