#!/usr/bin/env bash
# Build the bundled wasm guests and install bundled packs into a mods root,
# the layout the game discovers: <root>/<id>/ holds the pack files plus the
# compiled guest as mod.wasm.
#
#   bash scripts/install-mods.sh [--profile <profile>] <mods-root> [mod-id...]
#
# Further arguments name the mods to build and install; with none, build and
# install every bundled mod.
#
# The single implementation of the bundled-pack convention. `make mods` /
# `make mod ID=` install into mods/, with-test-mods.sh and CI stage a root for
# the tests' PETRAMOND_MODS, and the release workflow stages the shipped
# bundle:
#   - crate name == directory name == the mod id: mods-src/<id>/pack/pack.json
#     makes <id> a bundled pack, and the pack's "id" must be <id>;
#   - a pack whose directory is a crate is a wasm mod: its compiled guest
#     (<id> with '-' as '_', .wasm) must exist, and its pack.json must name
#     mod.wasm as "wasm" and/or "client_wasm";
#   - a pack directory that is not a crate is content-only, and its pack.json
#     must not name a guest;
#   - crates without pack/ (shared libraries, test support) are built but
#     never installed;
# Downloadable addons live in the separate petramond-addons repository and
# are built by `make addons`.
#
# Profile: `--profile <p>`, else $MOD_PROFILE, else `wasm-dev`.
#   wasm-dev   fast iteration build (no LTO, opt-level 2): `make mods`, local runs
#   wasm-test  release's opt-level and panic=abort minus the LTO: the test suite
#   release    the shipping build: fat LTO, one codegen unit
# Only files that differ from what is already installed are copied, so
# re-installing an unchanged mod leaves its directory untouched.
set -euo pipefail

profile=${MOD_PROFILE:-wasm-dev}
while (($# > 0)); do
    case $1 in
        --profile)
            (($# >= 2)) || break
            profile=$2
            shift 2
            ;;
        *) break ;;
    esac
done
if (($# < 1)); then
    echo "usage: $0 [--profile <profile>] <mods-root> [mod-id...]" >&2
    exit 2
fi
dest=$1
shift

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# CARGO_CMD may be multi-word (e.g. `nice -n 10 cargo`), so split it into words once.
IFS=' ' read -r -a cargo_cmd <<< "${CARGO_CMD:-cargo}"
wasm_target=wasm32-unknown-unknown
target_dir="$repo_root/target"
case $profile in
    dev) profile_dir=debug ;;
    *) profile_dir=$profile ;;
esac
wasm_dir="$target_dir/$wasm_target/$profile_dir"

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
        crate_dir=$(dirname "$pack_source")
        mod_ids+=("$(basename "$crate_dir")")
    done
    package_args=(--workspace)
fi

# Cargo reads .cargo/config.toml from the directory it runs in, not from the
# manifest's: build from inside mods-src so its guest flags (+simd128) apply.
# A RUSTFLAGS in the environment replaces them, which the engine would then
# run without SIMD.
if ((${#package_args[@]} > 0)); then
    (
        cd "$repo_root/mods-src"
        "${cargo_cmd[@]}" build \
            --target-dir "$target_dir" \
            --profile "$profile" \
            --target "$wasm_target" \
            "${package_args[@]}"
    )
fi

# The first "field": "value" pair for $1 in pack.json $2 (empty when absent).
pack_field() {
    grep -Eo "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" "$2" | head -n 1 | sed -E 's/.*"([^"]*)"$/\1/' || true
}

fail() {
    echo "install-mods: $*" >&2
    exit 1
}

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
    manifest="$pack_source/pack.json"
    [[ -f $manifest ]] || continue

    declared_id=$(pack_field id "$manifest")
    [[ $declared_id == "$mod_id" ]] ||
        fail "mods-src/$mod_id/pack/pack.json declares id '$declared_id'; a bundled pack's id is its directory name"
    server_wasm=$(pack_field wasm "$manifest")
    client_wasm=$(pack_field client_wasm "$manifest")

    wasm_source=
    if [[ -f $crate_dir/Cargo.toml ]]; then
        wasm_source="$wasm_dir/${mod_id//-/_}.wasm"
        [[ -f $wasm_source ]] || fail "bundled pack '$mod_id' has no compiled guest at $wasm_source"
        [[ -n $server_wasm$client_wasm ]] ||
            fail "bundled pack '$mod_id' is a crate but its pack.json names no \"wasm\" or \"client_wasm\""
        for named in $server_wasm $client_wasm; do
            [[ $named == mod.wasm ]] ||
                fail "bundled pack '$mod_id' names guest '$named'; the installed guest is mod.wasm"
        done
    elif [[ -n $server_wasm$client_wasm ]]; then
        fail "content-only pack '$mod_id' (no Cargo.toml) names a wasm guest"
    fi

    changed=""
    while IFS= read -r -d '' file; do
        changed+=$(copy_if_changed "$file" "$dest/$mod_id/${file#"$pack_source"/}")
    done < <(find "$pack_source" -type f -print0)
    if [[ -n $wasm_source ]]; then
        changed+=$(copy_if_changed "$wasm_source" "$dest/$mod_id/mod.wasm")
        kind="$profile guest"
    else
        kind="content only"
    fi
    if [[ -n $changed ]]; then
        echo "installed $dest/$mod_id ($kind)"
    else
        echo "unchanged $dest/$mod_id ($kind)"
    fi
done
