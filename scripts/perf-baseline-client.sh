#!/usr/bin/env bash
# perf-baseline-client.sh: Run the replay smoke and dump the perf buffer
# to docs/perf-baselines/<branch>-<timestamp>.json.
#
# Requires `make dev-e2e` to be running in another shell (or starts it).
set -euo pipefail

BRANCH=$(git rev-parse --abbrev-ref HEAD)
TS=$(date -u +'%Y%m%d-%H%M%S')
OUT="docs/perf-baselines/${BRANCH}-${TS}.json"
mkdir -p docs/perf-baselines

cd daemon/frontend
pnpm exec playwright test e2e/replay-smoke.spec.ts --reporter=json > "../../${OUT}.playwright.json" 2>&1 || true

echo "perf-baseline-client: wrote ${OUT}.playwright.json" >&2
echo "perf-baseline-client: TODO PR-B will extract __ozmuxPerfBuffer and write ${OUT}" >&2
