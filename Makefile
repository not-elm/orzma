.PHONY: build dev-frontend dev-backend dev-daemon dev-e2e dev-e2e-setup dev-e2e-stop kill-daemon verify-out-dir clean help fix-lint test-frontend test-wire-goldens test-wire-contract memo-build-sdk

FRONTEND_DIR := daemon/frontend
HTTP_DIR := daemon/http_server/src/handlers
INDEX_HTML := $(HTTP_DIR)/index.html
OZMUX_EXTENSION_ROOT := $(CURDIR)/extensions

help:
	@echo "Targets:"
	@echo "  build              - Build frontend to single HTML, then build release binary"
	@echo "  dev-frontend       - Run vite dev server on :5173 with HMR"
	@echo "  dev-backend        - Run axum server on :3200 (debug build redirects / to :5173)"
	@echo "  dev-daemon         - Run daemon_bootstrap (extensions loaded; Chrome cookie import skipped)"
	@echo "  dev-e2e-setup      - One-time prerequisites for the Playwright UI verification harness"
	@echo "  dev-e2e            - Launch vite + daemon for Playwright MCP verification (waits for ready)"
	@echo "  dev-e2e-stop       - Stop the verification harness started by dev-e2e"
	@echo "  kill-daemon        - Kill the daemon listening on :3200 and any stray cef_host"
	@echo "  clean              - Remove frontend node_modules, entire cargo target (workspace-wide), and built index.html"

verify-out-dir:
	@stray=$$(find $(HTTP_DIR) -mindepth 1 -type f ! -name '*.rs' ! -name 'index.html' 2>/dev/null); \
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
	cargo build --release -p daemon_bootstrap

dev-frontend:
	pnpm --dir $(FRONTEND_DIR) dev

dev-backend:
	cargo run -p daemon_bootstrap

dev-daemon: memo-build-sdk
	OZMUX_EXTENSION_ROOT=$(OZMUX_EXTENSION_ROOT) \
	OZMUX_BROWSER_SKIP_COOKIE_IMPORT=1 \
	cargo run -p daemon_bootstrap

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

kill-daemon:
	@pids=$$(lsof -nP -iTCP:3200 -sTCP:LISTEN -t 2>/dev/null); \
	if [ -n "$$pids" ]; then \
		echo "killing daemon on :3200 (pid $$pids)"; \
		kill $$pids 2>/dev/null || true; \
	else \
		echo "no daemon listening on :3200"; \
	fi; \
	cef_pids=$$(pgrep -x cef_host 2>/dev/null); \
	helper_pids=$$(pgrep -x cef_helper 2>/dev/null); \
	all_pids="$$cef_pids $$helper_pids"; \
	if [ -n "$$(echo $$all_pids | tr -d ' ')" ]; then \
		echo "killing stray cef_host/cef_helper (pid $$all_pids)"; \
		kill $$all_pids 2>/dev/null || true; \
	fi

test-frontend:
	pnpm --dir $(FRONTEND_DIR) exec vitest run

test-wire-goldens:
	@for bin in daemon/terminal/tests/fixtures/wire_msgpack/*.bin; do \
		echo "verify $$bin"; \
		tools/bin-to-diag.sh "$$bin" | diff -u "$${bin%.bin}.diag.txt" -; \
	done

test-wire-contract:
	cargo run -p ozmux_terminal --example emit_fixture -- --all
	pnpm exec tsx tools/verify-msgpack.ts daemon/terminal/tests/fixtures/wire_msgpack/
