# Sourced by scripts that gate on specific, named tests. Expects `cargo_cmd`
# (the caller's CARGO_CMD split into words) to be set.
#
# `cargo test ... -- --exact <path>` exits 0 with "running 0 tests" when the
# path no longer names a test, so a rename or a moved module silently turns a
# named gate into a no-op. run_named_tests passes every name as an exact
# filter in ONE invocation and fails unless exactly that many tests passed.
#
#   run_named_tests <cargo test args...> -- [--libtest-flag...] <test path...>
#
# Arguments after `--` that start with `--` go to libtest as flags (e.g.
# `--ignored`, `--nocapture`, `--test-threads=1`); the rest are test paths.
# Select exactly one test binary (`--lib`) so each path can match only once.
run_named_tests() {
    local -a cargo_args=() libtest_flags=() names=()
    while (($#)) && [[ $1 != -- ]]; do
        cargo_args+=("$1")
        shift
    done
    if (($# == 0)); then
        echo "run_named_tests: missing '--' before the test paths" >&2
        return 2
    fi
    shift
    local arg
    for arg in "$@"; do
        if [[ $arg == --* ]]; then
            libtest_flags+=("$arg")
        else
            names+=("$arg")
        fi
    done
    if ((${#names[@]} == 0)); then
        echo "run_named_tests: no test paths given" >&2
        return 2
    fi

    local log
    log=$(mktemp "${TMPDIR:-/tmp}/petramond-named-tests.XXXXXXXX")
    local status=0
    # The `+` expansions keep empty arrays legal under `set -u` on old bash.
    "${cargo_cmd[@]}" test ${cargo_args[@]+"${cargo_args[@]}"} -- \
        --exact ${libtest_flags[@]+"${libtest_flags[@]}"} "${names[@]}" 2>&1 |
        tee "$log" || status=$?
    # libtest's summary line; a `--nocapture` test's own output cannot fake it
    # because only lines that start with the exact prefix are counted.
    local passed
    passed=$(sed -n 's/^test result: ok\. \([0-9][0-9]*\) passed;.*/\1/p' "$log" |
        awk '{ total += $1 } END { print total + 0 }')
    rm -f -- "$log"
    if ((status != 0)); then
        return "$status"
    fi
    if ((passed != ${#names[@]})); then
        printf 'named test gate: %d of %d tests ran; a path below no longer names a test:\n' \
            "$passed" "${#names[@]}" >&2
        printf '  %s\n' "${names[@]}" >&2
        return 1
    fi
}
