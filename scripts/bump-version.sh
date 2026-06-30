#!/usr/bin/env bash
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "Usage: $0 <new-version>" >&2
    exit 1
fi

NEW="$1"

echo "$NEW" > VERSION

VERSION_STR="$NEW" python3 -c "
import re, pathlib, os
new_ver = os.environ['VERSION_STR']
p = pathlib.Path('Cargo.toml')
txt = p.read_text()
txt = re.sub(r'^version = \"[^\"]+\"', f'version = \"{new_ver}\"', txt, count=1, flags=re.MULTILINE)
p.write_text(txt)
"

VERSION_STR="$NEW" python3 -c "
import re, pathlib, os
new_ver = os.environ['VERSION_STR']
p = pathlib.Path('sdk/ozma-web/package.json')
txt = p.read_text()
txt = re.sub(r'(\"version\"\s*:\s*)\"[^\"]+\"', rf'\g<1>\"{new_ver}\"', txt, count=1)
p.write_text(txt)
"

echo "Bumped to $NEW"
