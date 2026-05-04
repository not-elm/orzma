.PHONY: build dev-frontend dev-backend verify-out-dir clean help

FRONTEND_DIR := daemon/frontend
HTTP_DIR := daemon/core/src/http
INDEX_HTML := $(HTTP_DIR)/index.html

help:
	@echo "Targets:"
	@echo "  build              - Build frontend to single HTML, then build release binary"
	@echo "  dev-frontend       - Run vite dev server on :5173 with HMR"
	@echo "  dev-backend        - Run axum server on :3200 (debug build redirects / to :5173)"
	@echo "  clean              - Remove frontend node_modules, entire cargo target (workspace-wide), and built index.html"

verify-out-dir:
	@stray=$$(find $(HTTP_DIR) -mindepth 1 ! -name '*.rs' ! -name 'index.html' 2>/dev/null); \
	if [ -n "$$stray" ]; then \
		echo "ERROR: unexpected files in $(HTTP_DIR):"; \
		echo "$$stray"; \
		echo "vite-plugin-singlefile is supposed to inline everything; investigate."; \
		exit 1; \
	fi

build:
	pnpm --dir $(FRONTEND_DIR) install --frozen-lockfile
	pnpm --dir $(FRONTEND_DIR) build
	@$(MAKE) --no-print-directory verify-out-dir
	cargo build --release -p ozmux_core

dev-frontend:
	pnpm --dir $(FRONTEND_DIR) dev

dev-backend:
	cargo run -p ozmux_core

clean:
	rm -rf $(FRONTEND_DIR)/node_modules target $(INDEX_HTML)

fix-lint:
	cargo clippy --fix
	cargo fmt
