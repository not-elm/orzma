.PHONY: run build clean help fix-lint setup-cef ozmd ozmd-web

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
