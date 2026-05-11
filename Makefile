.PHONY: build dev-frontend dev-backend dev-daemon dev-e2e dev-e2e-setup dev-e2e-stop verify-out-dir clean help fix-lint test-frontend memo-build-sdk

FRONTEND_DIR := daemon/frontend
HTTP_DIR := daemon/core/src/http
INDEX_HTML := $(HTTP_DIR)/index.html
OZMUX_EXTENSION_ROOT := $(CURDIR)/extensions

help:
	@echo "Targets:"
	@echo "  build              - Build frontend to single HTML, then build release binary"
	@echo "  dev-frontend       - Run vite dev server on :5173 with HMR"
	@echo "  dev-backend        - Run axum server on :3200 (debug build redirects / to :5173)"
	@echo "  dev-daemon         - Run daemon_bootstrap with OZMUX_EXTENSION_ROOT=$(EXTENSIONS_DIR)"
	@echo "  dev-e2e-setup      - One-time prerequisites for the Playwright UI verification harness"
	@echo "  dev-e2e            - Launch vite + daemon for Playwright MCP verification (waits for ready)"
	@echo "  dev-e2e-stop       - Stop the verification harness started by dev-e2e"
	@echo "  clean              - Remove frontend node_modules, entire cargo target (workspace-wide), and built index.html"

verify-out-dir:
	@stray=$$(find $(HTTP_DIR) -mindepth 1 ! -name '*.rs' ! -name 'index.html' 2>/dev/null); \
	if [ -n "$$stray" ]; then \
		echo "ERROR: unexpected files in $(HTTP_DIR):"; \
		echo "$$stray"; \
		echo "vite-plugin-singlefile is supposed to inline everything; investigate."; \
		exit 1; \
	fi

memo-build-sdk:
	pnpm --filter memo run build:sdk

build:
	pnpm --dir $(FRONTEND_DIR) install --frozen-lockfile
	pnpm --dir $(FRONTEND_DIR) build
	@$(MAKE) --no-print-directory verify-out-dir
	cargo build --release -p ozmux_core

dev-frontend:
	pnpm --dir $(FRONTEND_DIR) dev

dev-backend:
	cargo run -p ozmux_core

dev-daemon: memo-build-sdk
	OZMUX_EXTENSION_ROOT=$(OZMUX_EXTENSION_ROOT) cargo run -p daemon_bootstrap

clean:
	rm -rf $(FRONTEND_DIR)/node_modules target $(INDEX_HTML)

fix-lint:
	cargo clippy --fix --allow-dirty --allow-staged
	cargo fmt
	pnpm lint:fix

dev-e2e-setup:
	./scripts/dev-e2e.sh setup

dev-e2e: memo-build-sdk
	./scripts/dev-e2e.sh start

dev-e2e-stop:
	./scripts/dev-e2e.sh stop

test-frontend:
	pnpm --dir $(FRONTEND_DIR) exec vitest run
