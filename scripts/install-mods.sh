#!/usr/bin/env bash
# Build bundled wasm guests from mods-src/ and install their packs (pack files
# + mod.wasm) into the directory given as $1, where the game (or the test
# suite, through PETRAMOND_MODS) discovers them. Further arguments name the
# mods to build and install; with none, every bundled mod is.
#
# Convention: crate name == directory name == the mod id in pack/pack.json.
# Crates without a pack/ dir (test fixtures, shared libraries) are built but
# not installed; a pack without a crate installs as content-only.
#
# MOD_PROFILE picks the cargo profile (default `wasm-dev`: fast to build, for
# tests and local runs; `release` is the fat-LTO packaging build). Only files
# that differ from what is already installed are copied, so re-installing an
# unchanged mod leaves its directory untouched.
set -euo pipefail

if (($# < 1)); then
    echo "usage: $0 <destination-dir> [mod-id...]" >&2
    exit 2
fi
dest=$1
shift

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# CARGO_CMD may be multi-word (e.g. `nice -n 10 cargo`), so split it into words once.
IFS=' ' read -r -a cargo_cmd <<< "${CARGO_CMD:-cargo}"
profile=${MOD_PROFILE:-wasm-dev}
wasm_target=wasm32-unknown-unknown
wasm_target_dir="$repo_root/target"
case $profile in
    dev) profile_dir=debug ;;
    *) profile_dir=$profile ;;
esac

if ! rustup target list --installed | grep -qx "$wasm_target"; then
    echo "missing Rust target '$wasm_target'; run: rustup target add $wasm_target" >&2
    exit 2
fi

if (($# > 0)); then
    mod_ids=("$@")
    package_args=()
    for mod_id in "${mod_ids[@]}"; do
        if [[ ! -d "$repo_root/mods-src/$mod_id" ]]; then
            echo "no mod '$mod_id' in mods-src/" >&2
            exit 2
        fi
        if [[ -f "$repo_root/mods-src/$mod_id/Cargo.toml" ]]; then
            package_args+=(-p "$mod_id")
        fi
    done
else
    mod_ids=()
    for pack_source in "$repo_root"/mods-src/*/pack; do
        mod_ids+=("$(basename "$(dirname "$pack_source")")")
    done
    package_args=(--workspace)
fi

if ((${#package_args[@]} > 0)); then
    "${cargo_cmd[@]}" build \
        --manifest-path "$repo_root/mods-src/Cargo.toml" \
        --target-dir "$wasm_target_dir" \
        --profile "$profile" \
        --target "$wasm_target" \
        "${package_args[@]}"
fi

# Copies $1 to $2 unless $2 already holds the same bytes; prints 1 if it copied.
copy_if_changed() {
    if [[ -f $2 ]] && cmp -s "$1" "$2"; then
        return
    fi
    mkdir -p "$(dirname "$2")"
    cp "$1" "$2"
    echo 1
}

mkdir -p "$dest"
for mod_id in "${mod_ids[@]}"; do
    crate_dir="$repo_root/mods-src/$mod_id"
    pack_source="$crate_dir/pack"
    [[ -f "$pack_source/pack.json" ]] || continue
    changed=""
    while IFS= read -r -d '' file; do
        changed+=$(copy_if_changed "$file" "$dest/$mod_id/${file#"$pack_source"/}")
    done < <(find "$pack_source" -type f -print0)
    if [[ -f "$crate_dir/Cargo.toml" ]]; then
        wasm_source="$wasm_target_dir/$wasm_target/$profile_dir/${mod_id//-/_}.wasm"
        if [[ ! -f $wasm_source ]]; then
            echo "bundled pack '$mod_id' has no compiled guest at $wasm_source" >&2
            exit 1
        fi
        changed+=$(copy_if_changed "$wasm_source" "$dest/$mod_id/mod.wasm")
    fi
    if [[ -n $changed ]]; then
        echo "installed $dest/$mod_id"
    else
        echo "unchanged $dest/$mod_id"
    fi
done
