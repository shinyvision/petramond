#!/usr/bin/env bash
# Print the shipped content pack ids from mods-src/*/pack/pack.json, sorted
# and comma-separated. A pack's id need not equal its crate directory name.
# The website's app.content.reserved-mod-ids is exactly this list.
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
python3 - "$repo_root"/mods-src/*/pack/pack.json <<'PY'
import json, os, sys

ids = []
for path in sys.argv[1:]:
    crate = os.path.dirname(os.path.dirname(path))
    with open(path, encoding="utf-8") as f:
        manifest = json.load(f)
    if not isinstance(manifest.get("id"), str):
        sys.exit(f"{path} has no id")
    ids.append(manifest["id"])
print(",".join(sorted(ids)))
PY
