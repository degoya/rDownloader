# shellcheck shell=bash
#
# The version-dependent tests, run with the version being released (RD-1300-06).
#
#   rd_version_tests <version>    # from the checkout root; JOBS caps the build and the threads
#
# The release chain's test step reuses a --full Rust green of the tree before the version bump when
# the bump changed version lines only (RD-140-06, rd_prebump_reuse). That green never ran a test
# that reads the version with the new one: 1.28.0 shipped with
# `the_baselines_are_the_last_two_releases` red on main. So after a reuse step_test runs the tests
# of scripts/lib/version-tests.list (the one place the set is held; a repo lint keeps it complete),
# one nextest run per line, with RD_RELEASE_VERSION set. A line whose filter runs no test fails
# (`--no-tests=fail`), so a renamed module cannot pass silently. The evidence is one line per run
# and the total: `version-dependent tests: N run in M runs with X.Y.Z, all passed`.
#
# Fails when any run fails, naming each; the release's test step is then red as after a full run.

rd_version_tests() {
    local version="$1" root="${ROOT:-$PWD}" jobs="${JOBS:-2}"
    local list="$root/scripts/lib/version-tests.list" output
    local file package target filter count runs=0 total=0 failed=()
    output="$(mktemp)"
    while read -r file package target filter; do
        [[ -z "$file" || "$file" == \#* ]] && continue
        local selection=()
        case "$target" in
            lib) selection=(--lib) ;;
            bin:?*) selection=(--bin "${target#bin:}") ;;
            test:?*) selection=(--test "${target#test:}") ;;
            *)
                echo "version-tests.list: '$target' is no test target ($file)" >&2
                failed+=("$file")
                continue
                ;;
        esac
        [[ -n "$filter" ]] || { echo "version-tests.list: no filter for $file" >&2; failed+=("$file"); continue; }
        runs=$((runs + 1))
        echo "==> version-dependent: $file (-p $package ${selection[*]} -E '$filter')"
        RD_RELEASE_VERSION="$version" CARGO_BUILD_JOBS="$jobs" cargo nextest run -p "$package" \
            "${selection[@]}" -E "$filter" --no-tests=fail --no-fail-fast --test-threads "$jobs" \
            < /dev/null > "$output" 2>&1 || failed+=("$file")
        cat "$output"
        # nextest's summary, colours dropped: `Summary [ 0.120s] 7 tests run: 7 passed, 41 skipped`.
        count="$(sed -E 's/\x1b\[[0-9;]*m//g' "$output" \
            | sed -nE 's/.*Summary \[.*\] ([0-9]+) tests? run.*/\1/p' | tail -n 1)"
        total=$((total + ${count:-0}))
    done < "$list"
    rm -f "$output"
    if [[ ${#failed[@]} -gt 0 ]]; then
        echo "version-dependent tests: ${#failed[@]} of $runs run(s) red with $version:" >&2
        printf '  %s\n' "${failed[@]}" >&2
        return 1
    fi
    if [[ "$runs" -eq 0 ]]; then
        echo "version-dependent tests: $list names none" >&2
        return 1
    fi
    echo "version-dependent tests: $total run in $runs runs with $version, all passed"
}
