#!/usr/bin/env bash
# Run a command with PETRAMOND_MODS pointing at a freshly built, temporary copy
# of the bundled packs, so tests never read a developer's mods/.
#
# The guests build with the `wasm-test` profile (no LTO) unless MODS_PROFILE
# says otherwise; `make profile` measures with the shipping `release` guests.
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

temp_base=${TMPDIR:-/tmp}
cleanup() {
    if [[ -n ${run_root:-} && -d $run_root ]]; then
        rm -rf -- "$run_root"
    fi
}
# A signal only exits here; the EXIT trap then cleans up on every path. The
# pid in the name lets the test processes' own sweep reap it after a SIGKILL.
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
run_root=$(mktemp -d "$temp_base/petramond-test-mods-$$.XXXXXX")
test_mod_root="$run_root/mods"
mkdir -p "$test_mod_root"

bash "$repo_root/scripts/install-mods.sh" --profile "${MODS_PROFILE:-wasm-test}" \
    "$test_mod_root"

export PETRAMOND_MODS="$test_mod_root"
# Every test process isolates its own data dir; sharing the compiled-module
# cache across them compiles each guest once per run instead of once per
# process.
export PETRAMOND_MODCACHE_DIR="$run_root/modcache"
cd "$repo_root"
"$@"
