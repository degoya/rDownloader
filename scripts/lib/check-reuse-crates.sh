# shellcheck shell=bash
# shellcheck disable=SC2034,SC2154  # narrows the scope check.sh and lib/check-tests.sh read
#
# A --full that builds on an earlier --full green per crate and per kind (RD-1150-06; owner,
# 2026-10-07: "wir waren doch schon grün und haben nur texte geändert, dazu müssen wir doch nicht
# erneut den kompletten Lauf machen"). lib/check-reuse.sh skips a half a green covers whole; this
# narrows a half it does not:
#
#   * Rust: against the recorded green of the Rust half (any checkout on the target) whose tree
#     differs least, every changed path the half reads is placed (lib/crate-graph.sh): the member
#     it lies in with its whole reverse hull, or only that member's tests for test code (tests/,
#     a #[cfg(test)] module file, an edit inside a #[cfg(test)] mod block). Those
#     members are tested in full, with the rd-api binaries and the crash runs among them. A
#     manifest, the lock file, the toolchain, cargo's configuration, a build script, rd-core, a
#     migration, or a path no crate is known to read runs the whole half.
#   * Web: when only translation catalogues (web/src/locales/<language>/*.json) changed since a
#     green of the web half, Vitest runs alone — every catalogue is loaded by its tests, and which
#     component uses a key cannot be told (keys are built at run time: `codes.${code}`). The
#     typecheck runs too when a source imports a changed catalogue, whose types it then reads.
#
# Why that proves what a whole run proves: every test outside the selection is the same test
# binary, built from the same sources and dependencies, that the green ran; it can only change
# through what it builds on, and the hull follows every path dependency, dev and build ones too.
# The honest limits, named in the job: features are unified over the selected members (`-p`), as
# at branch level, not over the workspace; a file another crate reads by a path it assembles from
# parts at run time is not seen. GitHub CI runs the whole workspace on every wave's integration.
#
# A run that passes records a --full green of the tree, which the tag and the packages accept.
# --again, any argument but --full, --rust and --web, or no usable green run everything.
# Expects check.sh's globals after rd_check_scope (`full`, `reuse`, `full_tree`, `run_rust`,
# `run_web`, `ROOT`), lib/verified.sh, lib/scope.sh, lib/crash-matrix.sh and lib/crate-graph.sh
# sourced, `skip`, and the working directory at the checkout root.

RD_LOCALE_CATALOGUE='^web/src/locales/[^/]+/[^/]+\.json$'

rd_check_reuse_crates() {
    crate_reuse_base=""
    web_locales_only=0
    [[ "$full" -eq 1 && "${reuse:-0}" -eq 1 && -n "$full_tree" ]] || return 0
    if [[ "$run_rust" -eq 1 ]]; then rd_reuse_rust_scope; fi
    if [[ "$run_web" -eq 1 ]]; then rd_reuse_web_scope; fi
}

# The recorded greens of half $1 that git can still compare with full_tree, as
# `<count> <tree>` lines, the fewest changed paths that half reads first.
rd_reuse_candidates() {
    local candidate changes
    while read -r candidate; do
        [[ -n "$candidate" ]] || continue
        changes="$(git diff --no-renames --name-only "$candidate" "$full_tree" 2> /dev/null)" || continue
        printf '%s %s\n' "$(rd_paths_read_by "$1" <<< "$changes" | grep -c . || true)" "$candidate"
    done < <(rd_full_recorded "$ROOT" "$1") | sort -n
}

# The changed paths half $1 reads between recorded tree $2 and full_tree.
rd_reuse_paths() {
    git diff --no-renames --name-only "$2" "$full_tree" | rd_paths_read_by "$1"
}

rd_reuse_rust_scope() {
    local count candidate paths demands wide="" kind package path name total
    local -a demanded_hull=() hull=() members=() own=() binaries=() suites=()
    echo
    rd_crate_load_graph
    while read -r count candidate; do
        [[ -n "$candidate" ]] || continue
        paths="$(rd_reuse_paths rust "$candidate")"
        demands="$(RD_CRATE_FROM="$candidate" RD_CRATE_TO="$full_tree" rd_crate_demands <<< "$paths")"
        if grep -q '^wide ' <<< "$demands"; then
            [[ -n "$wide" ]] || wide="since the green of tree ${candidate:0:12}, $(sed -n 's/^wide //p' <<< "$demands" | head -1)"
            continue
        fi
        crate_reuse_base="$candidate"
        break
    done < <(rd_reuse_candidates rust)
    if [[ -z "$crate_reuse_base" ]]; then
        echo "==> the Rust half per crate (RD-1150-06): the whole half — ${wide:-no --full green of it on this target}"
        return 0
    fi

    mapfile -t demanded_hull < <(sed -n 's/^hull \([^ ]*\) .*/\1/p' <<< "$demands" | LC_ALL=C sort -u)
    mapfile -t hull < <(rd_crate_reverse_hull "${demanded_hull[@]+"${demanded_hull[@]}"}")
    mapfile -t own < <(sed -n 's/^own \([^ ]*\) .*/\1/p' <<< "$demands" | LC_ALL=C sort -u \
        | grep -vxF -f <(printf '%s\n' "${hull[@]+"${hull[@]}"}" '') || true)
    mapfile -t members < <(printf '%s\n' "${hull[@]+"${hull[@]}"}" "${own[@]+"${own[@]}"}" | sed '/^$/d' | LC_ALL=C sort -u)
    total="${#RD_CRATE_NAME[@]}"

    wide_reason=""
    dependant_packages=()
    mapfile -t test_packages < <(printf '%s\n' "${members[@]+"${members[@]}"}" | grep -vx rd-api || true)
    test_packages_label="the change and what builds on it"

    # rd-api: built on the change, all of it; only its own test code changed, the binaries whose
    # files changed (crates/rd-api/tests/<binary>/) and its library for a test module in src/.
    rd_api_lib=0
    rd_api_selected=()
    rd_api_filter=()
    if printf '%s\n' "${hull[@]+"${hull[@]}"}" | grep -qx rd-api; then
        rd_api_lib=1
        rd_api_selected=("${rd_api_all[@]}")
        rd_api_reason="rd-api builds on the change since the green of tree ${crate_reuse_base:0:12}"
    elif printf '%s\n' "${own[@]+"${own[@]}"}" | grep -qx rd-api; then
        while read -r kind package path _; do
            [[ "$kind $package" == "own rd-api" ]] || continue
            if [[ "$path" =~ ^crates/rd-api/tests/([^/]+)/ && -f "crates/rd-api/tests/${BASH_REMATCH[1]}/main.rs" ]]; then
                binaries+=("${BASH_REMATCH[1]}")
            elif [[ "$path" == crates/rd-api/src/* ]]; then
                rd_api_lib=1
            else
                rd_api_lib=1
                mapfile -t binaries < <(rd_api_test_binaries)
                break
            fi
        done <<< "$demands"
        mapfile -t suites < <(rd_api_test_suites | awk -v binaries=" ${binaries[*]+"${binaries[*]}"} " 'index(binaries, " " $2 " ") { print $1 }')
        rd_api_selected=("${suites[@]+"${suites[@]}"}")
        rd_api_reason="their test files changed since the green of tree ${crate_reuse_base:0:12}, so each runs whole"
    fi
    mapfile -t rd_api_binaries < <(rd_api_test_binaries_of "${rd_api_selected[@]+"${rd_api_selected[@]}"}")
    rd_api_skip_reason="the --full green of tree ${crate_reuse_base:0:12} covers them; nothing they build on changed since"
    rd_api_lib_skip_reason="$rd_api_skip_reason"

    # The crash runs of the members selected, all of them when the list itself changed.
    crash_packages=()
    failpoints=0
    if git diff --no-renames --name-only "$crate_reuse_base" "$full_tree" | grep -qxF "$RD_CRASH_MATRIX_LIST"; then
        failpoints=1
    else
        mapfile -t crash_packages < <(rd_reuse_crash_packages "$demands" "${hull[@]+"${hull[@]}"}")
        [[ ${#crash_packages[@]} -eq 0 ]] || failpoints=1
    fi
    crash_skip_reason="no member it runs builds on the change since the --full green of tree ${crate_reuse_base:0:12}"

    # The sqlx offline data, when a selected member builds on sqlx.
    sqlx=0
    for name in "${members[@]+"${members[@]}"}"; do
        if grep -qE '^sqlx([. ]|$)' "${RD_CRATE_DIR[$name]}/Cargo.toml" 2> /dev/null; then sqlx=1; fi
    done
    sqlx_skip_reason="no member that builds on sqlx builds on the change since the --full green of tree ${crate_reuse_base:0:12}"

    echo "==> the Rust half per crate (RD-1150-06): builds on the --full green of tree ${crate_reuse_base:0:12}"
    echo "    changed since, read by the Rust half: $(grep -c . <<< "$paths" || true) path(s)"
    { grep -E '^(hull|own) ' <<< "$demands" || true; } | head -n 12 | while read -r kind package path _; do
        if [[ "$kind" == hull ]]; then echo "      $path -> $package and what builds on it"; else echo "      $path -> the tests of $package"; fi
    done
    echo "    checks ${#members[@]} of $total members: ${members[*]}"
    if [[ ${#rd_api_selected[@]} -gt 0 || "$rd_api_lib" -eq 1 ]]; then
        echo "    rd-api: $([[ "$rd_api_lib" -eq 1 ]] && echo 'the library, ')${#rd_api_binaries[@]} of $(rd_api_test_binaries | grep -c .) integration binaries"
    fi
    if [[ "$failpoints" -eq 1 ]]; then
        echo "    crash matrix: ${crash_packages[*]:-every run (scripts/lib/crash-matrix.list changed)}"
    fi
    echo "    the rest is the green's; \"skipped, and why\" lists it at the end (--again runs everything)"
    [[ "${#members[@]}" -eq "$total" ]] \
        || skip "tests of $((total - ${#members[@]})) of $total workspace members" "the --full green of tree ${crate_reuse_base:0:12} covers them; nothing they build on changed since (RD-1150-06)"
}

# The packages of scripts/lib/crash-matrix.list whose run the demands $1 call for, given the hull
# $2...: a package in the hull, always; one whose own tests changed, for the shared run, or for a
# run with a selection when a changed path is one of its `--test` binaries or test code they share
# (tests/ outside every binary).
rd_reuse_crash_packages() {
    local demands="$1" package selection dir kind owner path binary
    local -a hull=("${@:2}")
    while read -r package selection; do
        if [[ " ${hull[*]} " == *" $package "* ]]; then echo "$package"; continue; fi
        dir="${RD_CRATE_DIR[$package]:-}"
        [[ -n "$dir" ]] || continue
        while read -r kind owner path _; do
            [[ "$kind $owner" == "own $package" ]] || continue
            if [[ -z "$selection" ]]; then echo "$package"; break; fi
            [[ "$path" == "$dir/tests/"* ]] || continue
            binary="${path#"$dir/tests/"}"
            binary="${binary%%/*}"
            binary="${binary%.rs}"
            if [[ " $selection " == *" --test $binary "* ]] \
                || [[ ! -f "$dir/tests/$binary/main.rs" && ! -f "$dir/tests/$binary.rs" ]]; then
                echo "$package"
                break
            fi
        done <<< "$demands"
    done < <(rd_crash_matrix_rows)
}

rd_reuse_web_scope() {
    local count candidate paths imported="" path
    echo
    while read -r count candidate; do
        [[ -n "$candidate" && "$count" -gt 0 ]] || continue
        paths="$(rd_reuse_paths web "$candidate")"
        grep -qvE "$RD_LOCALE_CATALOGUE" <<< "$paths" && continue
        web_locales_only=1
        web_locales_base="$candidate"
        break
    done < <(rd_reuse_candidates web)
    if [[ "$web_locales_only" -eq 0 ]]; then
        echo "==> the web half per kind (RD-1150-06): the whole half — no web green on this target differs from this tree in translation catalogues only"
        return 0
    fi
    # A catalogue a source imports by name is typed from its content (vue-tsc reads it).
    web_locales_typecheck=0
    while read -r path; do
        if grep -rqF --include='*.ts' --include='*.vue' "${path#web/src/}" web/src; then imported+="${imported:+ }${path#web/src/}"; fi
    done <<< "$paths"
    [[ -z "$imported" ]] || web_locales_typecheck=1
    echo "==> the web half per kind (RD-1150-06): builds on the web green of tree ${web_locales_base:0:12}"
    echo "    only translation catalogues changed since: $(grep -c . <<< "$paths") file(s)"
    echo "    Vitest runs whole$([[ -n "$imported" ]] && echo ", and the typecheck, which reads $imported")"
}
