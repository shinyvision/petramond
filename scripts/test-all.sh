#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cargo_bin=${CARGO:-cargo}
cd "$repo_root"

"$cargo_bin" test --workspace --all-targets
"$cargo_bin" test -p petramond-worldgen --features worldgen-tests --all-targets
"$cargo_bin" test --manifest-path mods-src/Cargo.toml --target-dir target --workspace --all-targets
"$cargo_bin" test --manifest-path mod-sdk/Cargo.toml --target-dir target --all-targets
"$cargo_bin" test --manifest-path gui-builder/Cargo.toml --target-dir target --all-targets
