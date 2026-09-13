#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# CARGO may be multi-word (Makefile default: nice -n 10 cargo), so split it into words once.
IFS=' ' read -r -a cargo_cmd <<< "${CARGO:-cargo}"
days=${SWEEP_DAYS:-3}
cd "$repo_root"

command -v cargo-sweep >/dev/null || {
    echo "sweep needs cargo-sweep: cargo install cargo-sweep" >&2
    exit 2
}

# Every package/feature selection builds its own copy of each crate, and cargo
# never deletes the old ones. cargo-sweep skips incremental/, which holds most
# of the bytes, but rustc rewrites a crate's incremental dir on every compile of
# it, so one untouched for $days days belongs to a build nobody is running.
removed_dirs=0
removed_kib=0
while IFS= read -r -d '' incremental; do
    mapfile -d '' stale < <(find "$incremental" -mindepth 1 -maxdepth 1 \
        -mmin +$((days * 1440)) -print0)
    ((${#stale[@]})) || continue
    kib=$(du -sck -- "${stale[@]}" | tail -1 | cut -f1)
    rm -rf -- "${stale[@]}"
    removed_dirs=$((removed_dirs + ${#stale[@]}))
    removed_kib=$((removed_kib + kib))
done < <(find . -type d -name incremental -path '*/target/*' -prune -print0)
printf 'sweep: removed %d incremental dirs untouched for %d days (%d MiB)\n' \
    "$removed_dirs" "$days" $((removed_kib / 1024))

# cargo-sweep ages a unit by the newest access time of any file in its
# .fingerprint dir. Cargo's own freshness check reads the hash file and only
# opens the .json on a rebuild, but repo-wide JSON searches read every .json
# and keep long-dead units looking used. Pin those access times to the write.
find . -path '*/target/*' -path '*/.fingerprint/*' -name '*.json' -print0 |
    python3 -c '
import os, sys
for path in sys.stdin.buffer.read().split(b"\0"):
    if path:
        mtime = os.stat(path).st_mtime_ns
        os.utime(path, ns=(mtime, mtime))
'

"${cargo_cmd[@]}" sweep --time "$days" --recursive .
