#!/usr/bin/env bash
# Build the bundled wasm guests into target/ and stage every bundled pack
# (pack files + mod.wasm) into the directory given as $1, which the test suite
# then reads through PETRAMOND_MODS. CI runs this once and shares the staged
# directory with every test job; with-test-mods.sh runs it into a temporary
# directory for local runs.
set -euo pipefail

if (($# != 1)); then
    echo "usage: $0 <destination-dir>" >&2
    exit 2
fi
dest=$1

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# CARGO_CMD may be multi-word (e.g. `nice -n 10 cargo`), so split it into words once.
IFS=' ' read -r -a cargo_cmd <<< "${CARGO_CMD:-cargo}"
wasm_target=wasm32-unknown-unknown
wasm_target_dir="$repo_root/target"

if ! rustup target list --installed | grep -qx "$wasm_target"; then
    echo "missing Rust target '$wasm_target'; run: rustup target add $wasm_target" >&2
    exit 2
fi

"${cargo_cmd[@]}" build \
    --manifest-path "$repo_root/mods-src/Cargo.toml" \
    --target-dir "$wasm_target_dir" \
    --release \
    --target "$wasm_target"

mkdir -p "$dest"
for pack_source in "$repo_root"/mods-src/*/pack; do
    [[ -f "$pack_source/pack.json" ]] || continue
    mod_id=$(basename "$(dirname "$pack_source")")
    wasm_name=${mod_id//-/_}.wasm
    wasm_source="$wasm_target_dir/$wasm_target/release/$wasm_name"
    mkdir -p "$dest/$mod_id"
    cp -R "$pack_source/." "$dest/$mod_id/"
    if [[ -f "$(dirname "$pack_source")/Cargo.toml" ]]; then
        if [[ ! -f $wasm_source ]]; then
            echo "bundled pack '$mod_id' has no compiled guest at $wasm_source" >&2
            exit 1
        fi
        cp "$wasm_source" "$dest/$mod_id/mod.wasm"
    fi
done
