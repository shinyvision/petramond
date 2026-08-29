#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# CARGO may be multi-word (Makefile default: nice -n 10 cargo), so split it into words once.
IFS=' ' read -r -a cargo_cmd <<< "${CARGO:-cargo}"
cd "$repo_root"

"${cargo_cmd[@]}" test --workspace --all-targets
"${cargo_cmd[@]}" test -p petramond-worldgen --features worldgen-tests --all-targets
"${cargo_cmd[@]}" test --manifest-path mods-src/Cargo.toml --target-dir target --workspace --all-targets
"${cargo_cmd[@]}" test --manifest-path mod-sdk/Cargo.toml --target-dir target --all-targets
"${cargo_cmd[@]}" test --manifest-path gui-builder/Cargo.toml --target-dir target --all-targets
