#!/usr/bin/env bash
# shellcheck shell=bash
#
# The crash and restart matrix from scripts/lib/crash-matrix.list (RD-191-09), whose header states
# the rules: the nextest runs, and the crates whose change at branch level calls for them.
# Sourced by scripts/check.sh and by the `crash-matrix` job of .github/workflows/ci.yml; both
# expect the working directory at the checkout root.
#
#   source scripts/lib/crash-matrix.sh
#   rd_crash_matrix_runs        # one line of nextest arguments per run, the shared run first,
#                               # filtered to the feature's tests
#   rd_crash_matrix_triggers    # the crates that own crash points, one per line, sorted
#   rd_crash_matrix_ci          # ci.yml's `crash-matrix` step: every run, each to its end

RD_CRASH_MATRIX_LIST="${RD_CRASH_MATRIX_LIST:-scripts/lib/crash-matrix.list}"

# The rows of the list: `<package> [<selection>]`, comments and blank lines removed.
rd_crash_matrix_rows() {
    grep -vE '^[[:space:]]*(#|$)' "$RD_CRASH_MATRIX_LIST"
}

# The shared run's selection (RD-1120-08): only the tests the feature turns on, not the listed
# crates' whole suites a second time — 1698 tests where about 45 change with it. A test behind
# `failpoints` carries `crash` in its path (its binary, module or name; rd-core's own cases sit in
# `failpoint`), which scripts/tests/crash-matrix.sh holds every listed crate to. One word, no
# space and no quote: the callers split the line on blanks.
RD_CRASH_MATRIX_FILTER='binary(/crash/)|test(/crash|failpoint/)'

rd_crash_matrix_runs() {
    local package selection features="" packages="" alone=()
    while read -r package selection; do
        if [[ -z "$selection" ]]; then
            features+="${features:+,}$package/failpoints"
            packages+="${packages:+ }-p $package"
        else
            alone+=("--features $package/failpoints -p $package $selection")
        fi
    done < <(rd_crash_matrix_rows)
    [[ -z "$packages" ]] || printf '%s\n' "--features $features $packages -E $RD_CRASH_MATRIX_FILTER"
    [[ ${#alone[@]} -eq 0 ]] || printf '%s\n' "${alone[@]}"
}

# The tests behind `failpoints` in the shared run's crates that RD_CRASH_MATRIX_FILTER would not
# select, one `<file>: <item>` per line; nothing when every one carries `crash` (or rd-core's
# `failpoint`). Read from the sources: a test file gated as a whole by its binary's name, a gated
# `mod` or `#[test]` function by its name or its file's (a crate root's names nothing).
rd_crash_matrix_unfiltered() {
    local package selection directory file binary
    while read -r package selection; do
        [[ -z "$selection" ]] || continue
        for directory in "crates/$package/src" "crates/$package/tests"; do
            [[ -d "$directory" ]] || continue
            while IFS= read -r file; do
                if [[ "$file" == crates/"$package"/tests/* ]] \
                    && grep -qE '^#!\[cfg\((all\()?(test, )?feature = "failpoints"' "$file"; then
                    binary="${file#crates/"$package"/tests/}"
                    binary="${binary%%/*}"
                    [[ "${binary%.rs}" =~ crash ]] || printf '%s: the whole file\n' "$file"
                    continue
                fi
                awk -v file="$file" '
                    function reset() { gated = 0; scoped = 0; is_test = 0 }
                    BEGIN { reset(); stem = file; sub(/.*\//, "", stem) }
                    /^[[:space:]]*#\[cfg\(/ && /feature = "failpoints"/ && !/not\(feature/ {
                        gated = 1; scoped = /test/; next
                    }
                    /^[[:space:]]*#\[(tokio::)?test/ { is_test = 1; next }
                    /^[[:space:]]*(#\[|\/\/)/ { next }
                    gated {
                        name = ""
                        if (match($0, /mod [a-z0-9_]+/) && scoped) name = substr($0, RSTART + 4, RLENGTH - 4)
                        else if (is_test && match($0, /fn [a-z0-9_]+/)) name = substr($0, RSTART + 3, RLENGTH - 3)
                        # The file names a module of the path too, unless it is a crate root.
                        if (name != "" && name !~ /crash|failpoint/ \
                            && (stem ~ /^(lib|main|mod)\.rs$/ || stem !~ /crash|failpoint/)) {
                            print file ": " name
                        }
                    }
                    { reset() }
                ' "$file"
            done < <(find "$directory" -name '*.rs' | LC_ALL=C sort)
        done
    done < <(rd_crash_matrix_rows)
}

# The `failpoints` feature list of package $1, one entry per line (`rd-core/failpoints`).
rd_crash_matrix_feature() {
    sed -n 's/^failpoints *= *\[\(.*\)\]/\1/p' "crates/$1/Cargo.toml" | tr ',' '\n' \
        | sed 's/[" ]//g; /^$/d'
}

# A package owns crash points when its feature turns rd-core's on (rd-core itself does); one run
# with a selection may only drive another crate's points, which its feature forwards to: rd-api's
# turns on rd-api-admin's, whose points the admin suite drives. Both kinds count.
rd_crash_matrix_triggers() {
    local package selection entry
    while read -r package selection; do
        if [[ "$package" == rd-core ]] || rd_crash_matrix_feature "$package" | grep -qx 'rd-core/failpoints'; then
            printf '%s\n' "$package"
        fi
        while read -r entry; do
            [[ "$entry" == rd-core/failpoints ]] || printf '%s\n' "${entry%/failpoints}"
        done < <(rd_crash_matrix_feature "$package")
    done < <(rd_crash_matrix_rows) | LC_ALL=C sort -u
}

# Every run under nextest's `ci` profile, as the `crash-matrix` job of ci.yml runs them: `-P ci`
# like `rust`'s tests, since that job builds no components either, every run to its end, and the
# flaky cases of each run's JUnit report as warnings (RD-191-09). Fails when any run failed.
rd_crash_matrix_ci() {
    local line python failed=0
    local -a runs run
    mapfile -t runs < <(rd_crash_matrix_runs)
    python="$(command -v python3 || command -v python)"
    for line in "${runs[@]}"; do
        read -r -a run <<< "${line}"
        # A run that fails before writing its report must not leave the previous run's.
        rm -f target/nextest/ci/junit.xml
        cargo nextest run -P ci --no-fail-fast "${run[@]}" || failed=1
        "${python}" scripts/lib/junit-flaky.py target/nextest/ci/junit.xml
    done
    return "${failed}"
}
