#!/usr/bin/env bash
# Run a command with PETRAMOND_MODS pointing at a freshly built, temporary copy
# of the bundled packs, so tests never read a developer's mods/.
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

temp_base=${TMPDIR:-/tmp}
test_mod_root=$(mktemp -d "$temp_base/petramond-test-mods.XXXXXX")
cleanup() {
    if [[ -n ${test_mod_root:-} && -d $test_mod_root ]]; then
        rm -rf -- "$test_mod_root"
    fi
}
trap cleanup EXIT INT TERM

bash "$repo_root/scripts/stage-test-mods.sh" "$test_mod_root"

export PETRAMOND_MODS="$test_mod_root"
cd "$repo_root"
"$@"
