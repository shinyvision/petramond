#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cargo_bin=${CARGO:-cargo}
wasm_target=wasm32-unknown-unknown
wasm_target_dir="$repo_root/target"

if ! rustup target list --installed | grep -qx "$wasm_target"; then
    echo "missing Rust target '$wasm_target'; run: rustup target add $wasm_target" >&2
    exit 2
fi

"$cargo_bin" build \
    --manifest-path "$repo_root/mods-src/Cargo.toml" \
    --target-dir "$wasm_target_dir" \
    --release \
    --target "$wasm_target"

temp_base=${TMPDIR:-/tmp}
test_mod_root=$(mktemp -d "$temp_base/petramond-test-mods.XXXXXX")
cleanup() {
    if [[ -n ${test_mod_root:-} && -d $test_mod_root ]]; then
        rm -rf -- "$test_mod_root"
    fi
}
trap cleanup EXIT INT TERM

for pack_source in "$repo_root"/mods-src/*/pack; do
    [[ -f "$pack_source/pack.json" ]] || continue
    mod_id=$(basename "$(dirname "$pack_source")")
    wasm_name=${mod_id//-/_}.wasm
    wasm_source="$wasm_target_dir/$wasm_target/release/$wasm_name"
    mkdir -p "$test_mod_root/$mod_id"
    cp -R "$pack_source/." "$test_mod_root/$mod_id/"
    if [[ -f "$(dirname "$pack_source")/Cargo.toml" ]]; then
        if [[ ! -f $wasm_source ]]; then
            echo "bundled pack '$mod_id' has no compiled guest at $wasm_source" >&2
            exit 1
        fi
        cp "$wasm_source" "$test_mod_root/$mod_id/mod.wasm"
    fi
done

export PETRAMOND_MODS="$test_mod_root"
cd "$repo_root"
"$@"
