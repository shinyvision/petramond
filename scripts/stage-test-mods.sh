#!/usr/bin/env bash
# Build the bundled wasm guests into target/ and stage every bundled pack
# (pack files + mod.wasm) into the directory given as $1, which the test suite
# then reads through PETRAMOND_MODS. CI runs this once and shares the staged
# directory with every test job; with-test-mods.sh runs it into a temporary
# directory for local runs. Tests use the fast `wasm-dev` guest profile unless
# MOD_PROFILE says otherwise.
set -euo pipefail

if (($# != 1)); then
    echo "usage: $0 <destination-dir>" >&2
    exit 2
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
MOD_PROFILE=${MOD_PROFILE:-wasm-dev} bash "$repo_root/scripts/install-mods.sh" "$1"
