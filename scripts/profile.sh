#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# CARGO_CMD may be multi-word (e.g. `nice -n 10 cargo`), so split it into words once.
IFS=' ' read -r -a cargo_cmd <<< "${CARGO_CMD:-cargo}"
profile_data=$(mktemp -d "${TMPDIR:-/tmp}/petramond-profile.XXXXXXXX")
cleanup() {
    rm -rf -- "$profile_data"
}
trap cleanup EXIT

cd "$repo_root"
# shellcheck source=lib/named-tests.sh
source "$repo_root/scripts/lib/named-tests.sh"
export PETRAMOND_DATA_DIR=$profile_data
export PETRAMOND_JOIN_RD=${PETRAMOND_JOIN_RD:-4}

# Run in the playtest profile because these are measurements, not correctness
# gates. The canonical test suite remains the explicit debug-safe test profile.
# One harness per invocation keeps each one's timings free of the other's load.
run_named_tests --profile playtest -p petramond-client --lib -- \
    --ignored --nocapture --test-threads=1 \
    game::tests::joinprofile::join_profile_sync
run_named_tests --profile playtest -p petramond-client --lib -- \
    --ignored --nocapture --test-threads=1 \
    app::tests::perf::world_map_zoom_out_frame_profile
