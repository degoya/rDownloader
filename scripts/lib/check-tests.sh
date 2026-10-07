#!/usr/bin/env bash
# shellcheck shell=bash
# shellcheck disable=SC2154  # the scope variables are check.sh's, which sources this file
#
# The Rust tests of scripts/check.sh, kept here so check.sh stays readable (RD-1100-12 T16): the
# workspace or the touched crates and one level of reverse dependencies, rd-api's library and
# the integration suites the change maps to in batches, the crash matrix and the sqlx offline
# data — each command through `attempt`, so a failure is listed and the run goes on.
#
# Expects from the caller what check.sh's scope section computes (`wide_reason`, `test_packages`,
# `dependant_packages`, `rd_api_selected`, `rd_api_all`, `rd_api_binaries`, `rd_api_filter`,
# `rd_api_reason`, `RD_API_MAP`, `failpoints`, `crash_triggers`, `sqlx`), `run_tests`, `step`,
# `skip` and `attempt`, rd_crash_matrix_runs from lib/crash-matrix.sh, and the working directory
# at the checkout root. A --full that builds on a green per crate (lib/check-reuse-crates.sh) also
# sets `test_packages_label`, `rd_api_lib`, `crash_packages` and the `*_skip_reason`s.

rd_check_rust_tests() {
    local crate binary batch_index batch_count crash_line
    local -a args=() batch=() names=() crash_runs=() crash_run=()
    if [[ -n "$wide_reason" ]]; then
        step "tests (everything except rd-api) — $wide_reason"
        attempt run_tests --workspace --exclude rd-api
    else
        if [[ ${#test_packages[@]} -gt 0 ]]; then
            step "tests (${test_packages_label:-touched}: ${test_packages[*]})"
            args=()
            for crate in "${test_packages[@]}"; do args+=(-p "$crate"); done
            attempt run_tests "${args[@]}"
        else
            skip "tests of the touched crates" "no crate under crates/ other than rd-api was ${test_packages_label:-touched}"
        fi
        if [[ ${#dependant_packages[@]} -gt 0 ]]; then
            step "tests (one level of reverse dependencies, library and binaries: ${#dependant_packages[@]} crates)"
            echo "    ${dependant_packages[*]}"
            # `--lib` fails outright when no selected package has a library (rdownloader,
            # rd-capture), so it is only asked for when one does.
            args=(--bins)
            for crate in "${dependant_packages[@]}"; do
                args+=(-p "$crate")
                if [[ -f "crates/$crate/src/lib.rs" ]] || grep -q '^\[lib\]' "crates/$crate/Cargo.toml" 2> /dev/null; then
                    [[ " ${args[*]} " == *" --lib "* ]] || args+=(--lib)
                fi
            done
            attempt run_tests "${args[@]}"
            skip "integration tests of ${#dependant_packages[@]} reverse dependencies" "branch level"
        fi
    fi

    # rd-api is split out because its integration binaries each link the entire dependency
    # graph, and a plain `--workspace` run builds all of them at once. That has OOM-killed
    # WSL even at JOBS=2: lowering the job count does not make a single link cheaper, so
    # the fix is to build fewer binaries at a time rather than to build them more slowly.
    if [[ "${rd_api_lib:-1}" -eq 1 ]]; then
        step "tests (rd-api library)"
        attempt run_tests -p rd-api --lib
    else
        skip "tests (rd-api library)" "${rd_api_lib_skip_reason:-}"
    fi

    if [[ ${#rd_api_selected[@]} -gt 0 ]]; then
        echo
        echo "==> rd-api integration: ${#rd_api_selected[@]} of ${#rd_api_all[@]} suites in ${#rd_api_binaries[@]} binaries — $rd_api_reason"
        batch=()
        names=()
        batch_index=0
        batch_count=$(( (${#rd_api_binaries[@]} + 3) / 4 ))
        for binary in "${rd_api_binaries[@]}"; do
            batch+=(--test "$binary")
            names+=("$binary")
            # Four binaries per batch: eight entries, each contributing `--test NAME`.
            if [[ ${#batch[@]} -ge 8 ]]; then
                batch_index=$((batch_index + 1))
                step "tests (rd-api integration, batch $batch_index of $batch_count: ${names[*]})"
                attempt run_tests -p rd-api "${batch[@]}" "${rd_api_filter[@]+"${rd_api_filter[@]}"}"
                batch=()
                names=()
            fi
        done
        if [[ ${#batch[@]} -gt 0 ]]; then
            batch_index=$((batch_index + 1))
            step "tests (rd-api integration, batch $batch_index of $batch_count: ${names[*]})"
            attempt run_tests -p rd-api "${batch[@]}" "${rd_api_filter[@]+"${rd_api_filter[@]}"}"
        fi
    fi
    if [[ ${#rd_api_selected[@]} -lt ${#rd_api_all[@]} ]]; then
        skip "$(( ${#rd_api_all[@]} - ${#rd_api_selected[@]} )) of ${#rd_api_all[@]} rd-api integration suites" \
            "${rd_api_skip_reason:-the change does not map to them ($RD_API_MAP)}"
    fi

    if [[ "$failpoints" -eq 1 ]]; then
        step "crash and restart matrix"
        # Off in every other run, including the one above: with the feature disabled the
        # crash points expand to nothing, which is the point. See
        # crates/rd-core/recovery-matrix.md, and scripts/lib/crash-matrix.list for why every
        # owning crate's own feature is turned on, not only rd-core's.
        # Read first and run after: a test reading stdin must not eat the next run's line.
        mapfile -t crash_runs < <(rd_crash_matrix_runs "${crash_packages[@]+"${crash_packages[@]}"}")
        for crash_line in "${crash_runs[@]}"; do
            read -r -a crash_run <<< "$crash_line"
            attempt run_tests "${crash_run[@]}"
        done
    else
        skip "crash and restart matrix" "${crash_skip_reason:-none of ${crash_triggers[*]}, failpoint.rs, the recovery matrix or scripts/lib/crash-matrix.list changed}"
    fi

    if [[ "$sqlx" -eq 1 ]]; then
        step "sqlx offline data"
        if cargo sqlx --version > /dev/null 2>&1; then
            # sqlx-cli 0.9 wants a database URL even offline; see .github/workflows/ci.yml.
            attempt env SQLX_OFFLINE=true DATABASE_URL="${DATABASE_URL:-sqlite::memory:}" \
                cargo sqlx prepare --check --workspace
        else
            echo "    sqlx-cli not installed; skipping (install: cargo install sqlx-cli --version 0.9.0 \\"
            echo "      --locked --no-default-features --features sqlite-unbundled)"
            skip "sqlx offline data" "sqlx-cli is not installed"
        fi
    else
        skip "sqlx offline data" "${sqlx_skip_reason:-no rd-db source and no .sql file changed}"
    fi
}
