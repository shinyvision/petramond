#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

# A missing rg empties the file list and the audit silently guards nothing.
command -v rg >/dev/null || {
    echo "source audit needs ripgrep (rg) to enumerate sources" >&2
    exit 2
}

# Raw line counts make test-heavy modules look much worse than the code that
# ships. Measure only the portion before a conventional trailing test module,
# and ignore dedicated test files. The host protocol is one declarative wire
# enum whose variants must stay together for postcard compatibility.
max_production_lines=1500
largest_lines=0
largest_file=
failed=0

while IFS= read -r file; do
    case "$file" in
        */tests/* | */tests.rs | src/world/relocated_world_crate_tests.rs)
            continue
            ;;
        mod-api/src/protocol/host.rs)
            continue
            ;;
    esac

    test_module_line=$(awk '
        previous_was_test_cfg && /^mod tests[[:space:]]*\{/ {
            print NR - 2
            exit
        }
        { previous_was_test_cfg = ($0 == "#[cfg(test)]") }
    ' "$file")
    if [[ -n "$test_module_line" ]]; then
        production_lines=$test_module_line
    else
        production_lines=$(wc -l < "$file")
    fi

    if (( production_lines > largest_lines )); then
        largest_lines=$production_lines
        largest_file=$file
    fi
    if (( production_lines > max_production_lines )); then
        printf 'production module exceeds %d lines: %s (%d)\n' \
            "$max_production_lines" "$file" "$production_lines" >&2
        failed=1
    fi
done < <(rg --files -g '*.rs' \
    src crates mods-src gui-builder mod-sdk mod-api petramond-ui petramond-text)

if (( failed )); then
    exit 1
fi
printf 'source audit: largest production module is %s (%d/%d lines)\n' \
    "$largest_file" "$largest_lines" "$max_production_lines"
