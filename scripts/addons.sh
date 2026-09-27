#!/usr/bin/env bash
# Build downloadable addons from their own checkout and pack them with the
# same archive writer and validator the content browser uses.
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
addons_dir="$repo_root/../petramond-addons"
content="$repo_root/target/playtest/petramond_content"
wasm_dir="$repo_root/target/wasm32-unknown-unknown/release"
out="$repo_root/output/addons"
content_packs=$(bash "$repo_root/scripts/content-pack-ids.sh")
IFS=' ' read -r -a cargo_cmd <<< "${CARGO_CMD:-cargo}"

[[ -f $addons_dir/Cargo.toml ]] || {
    echo "missing sibling addon checkout: $addons_dir" >&2
    exit 2
}

packages=()
crates=()
for crate in "$addons_dir"/*/; do
    [[ -f $crate/ADDON && -f $crate/pack/pack.json ]] || continue
    name=$(basename "$crate")
    packages+=(-p "$name")
    crates+=("$crate")
done
if ((${#crates[@]} == 0)); then
    echo "no addon crates found in $addons_dir" >&2
    exit 2
fi

(
    cd "$addons_dir"
    "${cargo_cmd[@]}" build --locked --target-dir "$repo_root/target" \
        --profile release --target wasm32-unknown-unknown "${packages[@]}"
)

for crate in "${crates[@]}"; do
    name=$(basename "$crate")
    wasm="$wasm_dir/${name//-/_}.wasm"
    [[ -f $wasm ]] || {
        echo "addon guest missing: $wasm" >&2
        exit 1
    }
    zip=$("$content" pack "$crate/pack" --wasm "$wasm" --out "$out" \
        --content-packs "$content_packs")
    echo "packed ${zip#"$repo_root"/}"
    if [[ ${INSTALL:-} == 1 ]]; then
        "$content" install "$zip" --kind addon
    fi
done
