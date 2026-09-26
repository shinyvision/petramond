#!/usr/bin/env bash
# The canonical debug-safe test suite, split into package groups so CI can run
# them as parallel jobs. With no arguments every group runs, each test once.
#
#   bash scripts/test-all.sh [group...]
#
# Groups (together they cover every test exactly once):
#   core        the root workspace minus the client-side crates, worldgen, the
#               GUI builder and the guest SDK
#   client      petramond-client, petramond-render, petramond-audio
#   worldgen    petramond-worldgen, its slow `#[ignore]`d sweeps included
#   mods        the mods-src wasm workspace (natively) and mod-sdk
#   gui-builder the GUI builder tool
# `workspace` is core + client in one cargo invocation (one feature
# resolution, so nothing compiles twice); the default local run uses it.
# `portable` is the headless engine/world/mesh subset CI also runs on Windows
# and macOS; it overlaps core by design (same tests, other platforms).
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# CARGO_CMD may be multi-word (e.g. `nice -n 10 cargo`), so split it into words once.
IFS=' ' read -r -a cargo_cmd <<< "${CARGO_CMD:-cargo}"
cd "$repo_root"

test_fast() {
    "${cargo_cmd[@]}" test --profile fasttest "$@"
}

# Worldgen runs only in its own group, which also runs its `#[ignore]`d slow
# sweeps (a runtime flag, so nothing recompiles), so the workspace groups
# leave it out.
client_packages=(-p petramond-client -p petramond-render -p petramond-audio)
client_excludes=(--exclude petramond-client --exclude petramond-render --exclude petramond-audio)
# Root workspace members with a group of their own: the builder (a desktop
# tool) and the guest SDK (tested with the mods it is linked into).
own_group_excludes=(--exclude petramond-worldgen --exclude gui-builder --exclude mod-sdk)

run_group() {
    case "$1" in
        workspace)
            test_fast --workspace "${own_group_excludes[@]}" --all-targets
            ;;
        core)
            test_fast --workspace "${own_group_excludes[@]}" "${client_excludes[@]}" --all-targets
            ;;
        client)
            test_fast "${client_packages[@]}" --all-targets
            ;;
        worldgen)
            test_fast -p petramond-worldgen --all-targets -- --include-ignored
            ;;
        mods)
            test_fast --manifest-path mods-src/Cargo.toml --target-dir target --workspace --all-targets
            test_fast -p mod-sdk --all-targets
            ;;
        gui-builder)
            test_fast -p gui-builder --all-targets
            ;;
        portable)
            test_fast -p petramond -p petramond-world -p petramond-mesh -p petramond-worldgen \
                -p petramond-util -p petramond-math -p petramond-region --all-targets
            ;;
        *)
            echo "unknown test group '$1' (see the header of $0)" >&2
            exit 2
            ;;
    esac
}

if (($# == 0)); then
    set -- workspace worldgen mods gui-builder
fi
for group in "$@"; do
    run_group "$group"
done
