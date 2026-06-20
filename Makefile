.PHONY: run build clean help fix-lint setup-cef ozmd ozmd-web setup-cef-release bundle-macos release-macos

OZMUX_EXTENSION_ROOT := $(CURDIR)/extensions
CARGO_BIN_DIR := $(if $(CARGO_HOME),$(CARGO_HOME)/bin,$(HOME)/.cargo/bin)

# bevy_cef (ozmux-gui) CEF integration. The version is pinned by bevy_cef's
# `cef` workspace dep; keep these in sync with /Users/watanabe/workspace/bevy_cef.
CEF_VERSION := 145.6.1+145.0.28
CEF_FRAMEWORK_LIB := $(HOME)/.local/share/cef/Chromium Embedded Framework.framework/Libraries
CEF_DEBUG_RENDER_PROCESS := bevy_cef_debug_render_process

help:
	@echo "Targets:"
	@echo "  run            - Run the ozmux-gui Bevy app (cargo run)"
	@echo "  build          - Build the workspace (cargo build)"
	@echo "  setup-cef      - Install the CEF framework + debug render process for ozmux-gui (macOS, one-time)"
	@echo "  setup-cef-release - Install arm64 CEF + release render process (for bundling)"
	@echo "  bundle-macos   - Build and package the ozmux .app (pass BUNDLE_ARGS=...)"
	@echo "  release-macos  - setup-cef-release then bundle with notarization"
	@echo "  fix-lint       - clippy --fix + rustfmt + biome lint:fix"
	@echo "  clean          - cargo clean (remove the workspace target dir)"
	@echo "  ozmd           - Build the web bundle then the ozmd binary"
	@echo "  ozmd-web       - Build the ozmd web bundle (esbuild)"

run:
	cargo run

build:
	cargo build

setup-cef:
	cargo install export-cef-dir@$(CEF_VERSION) --force
	export-cef-dir --force "$(HOME)/.local/share/cef"
	cargo install $(CEF_DEBUG_RENDER_PROCESS)
	cp "$(CARGO_BIN_DIR)/$(CEF_DEBUG_RENDER_PROCESS)" "$(CEF_FRAMEWORK_LIB)/$(CEF_DEBUG_RENDER_PROCESS)"

fix-lint:
	cargo clippy --workspace --fix --allow-dirty --allow-staged
	cargo fmt
	pnpm lint:fix

clean:
	cargo clean

ozmd-web:
	pnpm --filter @ozma/ozmd-web build

ozmd: ozmd-web
	cargo build -p ozmd

BEVY_CEF_RENDER_PROCESS := bevy_cef_render_process
BEVY_CEF_GIT := https://github.com/not-elm/bevy_cef
BEVY_CEF_BRANCH := passthrough

setup-cef-release:
	cargo install export-cef-dir@$(CEF_VERSION) --force
	export-cef-dir --force "$(HOME)/.local/share/cef"
	cargo install --git $(BEVY_CEF_GIT) --branch $(BEVY_CEF_BRANCH) $(BEVY_CEF_RENDER_PROCESS)

bundle-macos:
	python3 scripts/bundle_macos.py $(BUNDLE_ARGS)

release-macos: setup-cef-release
	python3 scripts/bundle_macos.py --notarize $(BUNDLE_ARGS)
