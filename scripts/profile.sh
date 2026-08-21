#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cargo_bin=${CARGO:-cargo}
profile_data=$(mktemp -d "${TMPDIR:-/tmp}/petramond-profile.XXXXXXXX")
cleanup() {
    rm -rf -- "$profile_data"
}
trap cleanup EXIT

cd "$repo_root"
export PETRAMOND_DATA_DIR=$profile_data
export PETRAMOND_JOIN_RD=${PETRAMOND_JOIN_RD:-4}

# Run in the playtest profile because these are measurements, not correctness
# gates. The canonical test suite remains the explicit debug-safe test profile.
"$cargo_bin" test --profile playtest -p petramond-client --lib \
    game::tests::joinprofile::join_profile_sync -- \
    --exact --ignored --nocapture --test-threads=1
"$cargo_bin" test --profile playtest -p petramond-client --lib \
    app::tests::perf::world_map_zoom_out_frame_profile -- \
    --exact --ignored --nocapture --test-threads=1
