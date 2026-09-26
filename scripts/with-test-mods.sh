#!/usr/bin/env bash
# Run a command with PETRAMOND_MODS pointing at a freshly built, temporary copy
# of the bundled packs, so tests never read a developer's mods/.
#
# The guests build with the `wasm-test` profile (no LTO) unless MODS_PROFILE
# says otherwise; `make profile` measures with the shipping `release` guests.
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

bash "$repo_root/scripts/install-mods.sh" --profile "${MODS_PROFILE:-wasm-test}" "$test_mod_root"

export PETRAMOND_MODS="$test_mod_root"
cd "$repo_root"
"$@"
