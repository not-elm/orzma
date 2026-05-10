# CLAUDE.md

Operational notes for Claude Code (the assistant) working in this repo.

## UI verification workflow

Use this when you have changed anything under `daemon/frontend/src/**`, the showcase, theme tokens, pane layout, or daemon-side endpoints that the UI consumes. Skip it for purely backend-internal changes that the UI does not exercise.

### First-time setup (per checkout)

1. Run prerequisites once:

   ```bash
   make dev-e2e-setup
   ```

   This installs JS dependencies, warms the Rust build cache, and downloads the Playwright Chromium binary.

2. In Claude Code, approve the project-scoped Playwright MCP server once:

   ```
   /mcp
   ```

   Approve the `playwright` server. The pinned version is `@playwright/mcp@0.0.75` with `--isolated --headless`.

### Verification loop

1. Start the harness in the background:

   ```bash
   make dev-e2e
   ```

   Wait for the single `ready` line on stdout. If it errors with "port already in use", inspect with `lsof -nP -iTCP:<port> -sTCP:LISTEN` and free the port before retrying.

2. Drive the browser via the Playwright MCP tools. Navigate to `http://localhost:5173`. Use `browser_snapshot` for DOM inspection, `browser_take_screenshot` for visual checks, and `browser_console_messages` to read errors.

3. When done, stop everything:

   ```bash
   make dev-e2e-stop
   ```

### Failure modes

| Symptom | Cause | Recovery |
| --- | --- | --- |
| `error: port 5173 is already in use.` | Stray Vite or another process | `lsof -nP -iTCP:5173 -sTCP:LISTEN`, kill the holder |
| `error: port 3200 is already in use.` | Stray daemon | same, for port 3200 |
| `error: harness already running (see .ozmux/e2e.pid).` | A previous harness is still up | `make dev-e2e-stop` |
| `error: readiness timeout after 30s.` | Vite or daemon failed to come up | Read the last 20 lines printed from `.ozmux/logs/vite.log` and `.ozmux/logs/daemon.log` |
| MCP tools missing or fail | Server not approved | Run `/mcp` and approve `playwright` |

### What lives where

- `scripts/dev-e2e.sh` — lifecycle script (start/stop/setup).
- `Makefile` — `dev-e2e`, `dev-e2e-setup`, `dev-e2e-stop` targets dispatch to the script.
- `.mcp.json` — registers `@playwright/mcp` (pinned).
- `.ozmux/` — runtime state (PID file, logs); gitignored.
