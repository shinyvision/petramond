#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# CARGO_CMD may be multi-word (e.g. `nice -n 10 cargo`), so split it into words once.
IFS=' ' read -r -a cargo_cmd <<< "${CARGO_CMD:-cargo}"
smoke_data=$(mktemp -d "${TMPDIR:-/tmp}/petramond-smoke.XXXXXXXX")
cleanup() {
    rm -rf -- "$smoke_data"
}
trap cleanup EXIT

cd "$repo_root"
# shellcheck source=lib/named-tests.sh
source "$repo_root/scripts/lib/named-tests.sh"
export PETRAMOND_DATA_DIR=$smoke_data

run_named_tests -p petramond --lib -- --test-threads=1 \
    server::handle::tests::spawned_server_ticks_answers_and_shuts_down_cleanly \
    server::remote::tests::headless_server_join_leave_cycle_freezes_the_world_when_empty
run_named_tests -p petramond-client --lib -- --test-threads=1 \
    app::tests::connect::end_to_end_connect_through_the_ui_joins_a_lan_server
