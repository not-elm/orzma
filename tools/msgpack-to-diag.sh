#!/usr/bin/env bash
# msgpack-to-diag.sh: decode a MessagePack .bin file into deterministic JSON-like text.
# Requirement: python3 + `pip install msgpack` (>= 1.0 for map order preservation).
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <path-to-.bin>" >&2
    exit 2
fi

python3 - "$1" <<'PY'
import json
import sys

import msgpack

with open(sys.argv[1], "rb") as f:
    data = msgpack.unpackb(f.read(), raw=False)

# Deterministic key order: encoded order is preserved by msgpack>=1.0 (dicts).
print(json.dumps(data, ensure_ascii=False, indent=2))
PY
