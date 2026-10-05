#!/usr/bin/env bash
# shellcheck shell=bash
# shellcheck disable=SC2034,SC2154  # sets the scope check.sh reads, from check.sh's change set
#
# What a scripts/check.sh run demands, from its change set, kept here so check.sh stays readable
# (RD-1100-12 T16): whether the change is documentation only or touches the Rust build, the
# touched crates and their reverse dependencies, a reason to run the whole workspace, the rd-api
# integration suites, and the triggers of the crash matrix, sqlx, the web half and the extension.
#
# Sets, as globals for check.sh and lib/check-tests.sh: docs_only, rust_touched, packages,
# wide_reason, lock_paths, test_packages, dependant_packages, RD_API_MAP, rd_api_all,
# rd_api_selected, rd_api_reason, rd_api_binaries, rd_api_filter, crash_triggers, failpoints,
# sqlx, web_changed, extension_changed. Expects `changed`, `boundary`, `full`, `ROOT` and
# `touches` as check.sh defines them, lib/scope.sh sourced, and the working directory at the
# checkout root.

rd_check_scope() {
    # crates/rd-core/recovery-matrix.md is deliberately NOT harmless text: a test compares it
    # against rd_core::failpoint::CRASH_POINTS, so editing it is a code change wearing a .md
    # extension. crates/rd-api/mcp-coverage.md is the same kind: rd-api's library include_str!s it
    # and mcp_coverage::doc_tests compares it with the capability table.
    docs_only=0
    if [[ -n "$changed" ]] && ! grep -qvE '^docs/|\.md$' <<< "$changed"; then
        docs_only=1
    fi
    if touches '^crates/rd-core/recovery-matrix\.md$|^crates/rd-api/mcp-coverage\.md$'; then docs_only=0; fi
    if [[ "$full" -eq 1 ]]; then docs_only=0; fi

    # Whether anything the Rust build reads changed at all. A web-only or scripts-only change
    # compiles nothing, so it gets no Rust test — not even rd-api's library.
    rust_touched=0
    if [[ "$full" -eq 1 ]] || touches '^crates/|^plugins/|^Cargo\.(toml|lock)$|^rust-toolchain\.toml$|\.sql$|^\.config/nextest\.toml$|^deny\.toml$'; then
        rust_touched=1
    fi

    mapfile -t crate_dirs < <(rd_scope_crate_dirs <<< "$changed")
    packages=()
    for directory in "${crate_dirs[@]+"${crate_dirs[@]}"}"; do
        name="$(rd_scope_package_name "$directory")"
        if [[ -n "$name" ]]; then packages+=("$name"); fi
    done

    # A change under plugins/ or to the WIT contract is guest code the contract tests in
    # rd-plugin-ext and rd-plugin-host run. Not in the plan's table, added here because without it a
    # scoped run of a plugin change would execute no Rust test at all.
    if touches '^plugins/|^crates/rd-plugin-api/wit/'; then
        packages+=(rd-plugin-ext rd-plugin-host)
    fi

    # Rust tests that read a file outside crates/ and plugins/ (RD-191-09): a catalogue rd-diagnostics
    # compiles in, the npm lockfile and the vendor texts the About page's licence list is held to
    # (RD-130-12), the container fixtures, a script a test pins. scripts/lib/rust-test-inputs.map
    # names the crates whose tests read each; they count as touched, which also makes the change a
    # Rust change.
    mapfile -t input_packages < <(rd_scope_input_packages <<< "$changed")
    if [[ ${#input_packages[@]} -gt 0 ]]; then
        packages+=("${input_packages[@]}")
        rust_touched=1
    fi

    # The whole workspace at branch level too, when the blast radius is everything: the toolchain,
    # the nextest or the deny configuration. Until RD-120-58 touching rd-core, rd-db, rd-scheduler or
    # rd-files, or more than six crates, did the same; now those get their own tests plus one level
    # of reverse dependencies, and the transitive rest waits for --full.
    #
    # The root Cargo.toml and Cargo.lock widened too until RD-130-17 — on 2026-09-24 once only for
    # a new dev-dependency. Now scripts/lib/lock-scope.py names the members whose resolved tree
    # changed, and they count as touched; a change it cannot narrow (a profile, a new member, a
    # [patch], anything outside [workspace.dependencies]) or a failure to answer still widens.
    wide_reason=""
    lock_paths=""
    governing="$(rd_scope_build_governing <<< "$changed")"
    if [[ -n "$governing" ]]; then
        wide_reason="$governing governs the whole build"
    elif [[ "$full" -eq 0 ]] && touches '^Cargo\.(toml|lock)$'; then
        lock_scope="$(rd_scope_lock_crates "$boundary")" \
            || lock_scope="WIDE scripts/lib/lock-scope.py could not answer"
        if [[ "$lock_scope" == WIDE\ * ]]; then
            wide_reason="Cargo.toml/Cargo.lock: ${lock_scope#WIDE }"
        else
            echo
            echo "==> Cargo.toml/Cargo.lock: the members whose resolved tree changed"
            plugin_members=0
            while read -r package directory why; do
                [[ -n "$package" ]] || continue
                if [[ "$directory" == plugins/* ]]; then
                    # Like a change under plugins/: the contract tests below, added once.
                    plugin_members=$((plugin_members + 1))
                    continue
                fi
                echo "    $package — $why"
                packages+=("$package")
                # For the rd-api map, the change counts as one to that crate's manifest.
                lock_paths+="$directory/Cargo.toml"$'\n'
            done <<< "$lock_scope"
            if [[ "$plugin_members" -gt 0 ]]; then
                echo "    $plugin_members plugin crates — their contract tests (rd-plugin-ext, rd-plugin-host)"
                packages+=(rd-plugin-ext rd-plugin-host)
            fi
            [[ -n "$lock_scope" ]] || echo "    none — the change resolves to the same tree"
        fi
    fi
    if [[ "$full" -eq 1 ]]; then wide_reason="--full"; fi

    # Branch level: the touched crates in full, and one level of reverse dependencies with their
    # library and binary tests only. rd-api is removed from both; it has a rule of its own below.
    test_packages=()
    dependant_packages=()
    if [[ -z "$wide_reason" && ${#packages[@]} -gt 0 ]]; then
        mapfile -t test_packages < <(printf '%s\n' "${packages[@]}" | sort -u | grep -vx rd-api || true)
        mapfile -t dependant_packages < <(printf '%s\n' "${packages[@]}" | rd_scope_reverse_deps \
            | sort -u | grep -vxF -f <(printf '%s\n' "${packages[@]}" rd-api) || true)
    fi

    # The rd-api integration suites: all of them in a wide run, otherwise what the map demands.
    # They run as the binaries holding them (RD-150-10), filtered to them when not all are selected.
    RD_API_MAP="scripts/lib/rd-api-tests.map"
    mapfile -t rd_api_all < <(rd_api_test_suites | cut -d' ' -f1)
    rd_api_selected=()
    rd_api_reason=""
    if [[ -n "$wide_reason" ]]; then
        rd_api_selected=("${rd_api_all[@]}")
        rd_api_reason="$wide_reason"
    elif [[ -n "$changed" ]]; then
        rd_api_demands="$(rd_api_test_demands "$RD_API_MAP" <<< "$changed"$'\n'"$lock_paths")"
        if grep -q '^all ' <<< "$rd_api_demands"; then
            rd_api_selected=("${rd_api_all[@]}")
            rd_api_reason="every suite, for $(grep '^all ' <<< "$rd_api_demands" | head -1 | cut -d' ' -f2-)"
        elif [[ -n "$rd_api_demands" ]]; then
            mapfile -t rd_api_selected < <(cut -d' ' -f1 <<< "$rd_api_demands" | LC_ALL=C sort -u)
            rd_api_reason="mapped from the change ($RD_API_MAP)"
        fi
    fi
    mapfile -t rd_api_binaries < <(rd_api_test_binaries_of "${rd_api_selected[@]+"${rd_api_selected[@]}"}")
    # With nextest, a partial selection runs only the selected suites' tests of those binaries.
    rd_api_filter=()
    if [[ ${#rd_api_selected[@]} -gt 0 && ${#rd_api_selected[@]} -lt ${#rd_api_all[@]} ]] \
        && command -v cargo-nextest > /dev/null; then
        rd_api_filter=(-E "$(rd_api_test_filter "${rd_api_selected[@]}")")
    fi

    # The crash matrix's runs and the crates that call for them: scripts/lib/crash-matrix.list, the
    # one list check.sh and ci.yml read (RD-191-09).
    # shellcheck source=lib/crash-matrix.sh
    source "$ROOT/scripts/lib/crash-matrix.sh"
    mapfile -t crash_triggers < <(rd_crash_matrix_triggers)
    failpoints=0
    if [[ "$full" -eq 1 ]] \
        || touches '^crates/rd-core/src/failpoint\.rs$|^crates/rd-core/recovery-matrix\.md$|^scripts/lib/crash-matrix\.' \
        || printf '%s\n' "${packages[@]+"${packages[@]}"}" | grep -qxF -f <(printf '%s\n' "${crash_triggers[@]}"); then
        failpoints=1
    fi

    sqlx=0
    if [[ "$full" -eq 1 ]] || touches '^crates/rd-db/|\.sql$'; then sqlx=1; fi

    web_changed=0
    if [[ "$full" -eq 1 ]] || touches '^web/'; then web_changed=1; fi

    extension_changed=0
    if [[ "$full" -eq 1 ]] || touches '^extension/'; then extension_changed=1; fi
}
