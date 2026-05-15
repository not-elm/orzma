#!/usr/bin/env bash
# bin-to-diag.sh: decode a .bin fixture into deterministic JSON-like text.
# Handles both MessagePack bins and JSON text bins (mode_change, hello).
# Requirement: python3 + `pip install msgpack` (>= 1.0 for map order preservation).
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <path-to-.bin>" >&2
    exit 2
fi

python3 - "$1" <<'PY'
import json
import sys

path = sys.argv[1]

with open(path, "rb") as f:
    raw = f.read()

# NOTE: JSON text bins start with '{'; msgpack maps start with 0x8x or 0xde/0xdf.
if raw[:1] == b'{':
    data = json.loads(raw)
else:
    import msgpack
    data = msgpack.unpackb(raw, raw=False)

print(json.dumps(data, ensure_ascii=False, indent=2))
PY
