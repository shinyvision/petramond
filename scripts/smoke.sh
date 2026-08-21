#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cargo_bin=${CARGO:-cargo}
smoke_data=$(mktemp -d "${TMPDIR:-/tmp}/petramond-smoke.XXXXXXXX")
cleanup() {
    rm -rf -- "$smoke_data"
}
trap cleanup EXIT

cd "$repo_root"
export PETRAMOND_DATA_DIR=$smoke_data

"$cargo_bin" test -p petramond --lib \
    server::handle::tests::spawned_server_ticks_answers_and_shuts_down_cleanly -- \
    --exact --test-threads=1
"$cargo_bin" test -p petramond-client --lib \
    app::tests::connect::end_to_end_connect_through_the_ui_joins_a_lan_server -- \
    --exact --test-threads=1
"$cargo_bin" test -p petramond --lib \
    server::remote::tests::headless_server_join_leave_cycle_freezes_the_world_when_empty -- \
    --exact --test-threads=1
