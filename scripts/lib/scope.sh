#!/usr/bin/env bash
# shellcheck shell=bash
#
# What a run has to check, derived from what the change actually touches.
#
# Kept out of scripts/check.sh so the decisions can be exercised on their own — every function
# here is pure apart from the `git` calls, takes its input as arguments or on stdin, and writes
# its answer to stdout. `bash -c 'source scripts/lib/scope.sh; rd_scope_crates <<< crates/rd-files/src/lib.rs'`
# is a complete test case.
#
# The honest limit, stated once here and again in AGENTS.md: the reverse-dependency step is ONE
# level, not a transitive hull. A crate three edges downstream of the change is not tested by a
# scoped run. That is what `--full` is for, and that is why the merge and the release chain run
# it rather than trusting a scoped green.

# The commit a scoped run diffs against: the OLDER of the branch point and the last green run.
#
# The branch point alone is empty on `development` itself, where every commit is already in the
# base — a run there would see only uncommitted work. The last green run alone would narrow the
# scope of a feature branch to whatever happened since. Taking the older of the two can only
# widen the change set, never narrow it, which is the direction a verification tool should err.
#
#   $1  the base branch (usually development)
#   $2  the checkout root, for the .rd-verified marker
rd_scope_boundary() {
    local base="$1" root="$2" boundary verified
    boundary="$(git merge-base HEAD "$base" 2> /dev/null || true)"
    [[ -n "$boundary" ]] || boundary="$(git rev-parse HEAD)"

    verified="$(rd_verified_revision "$root")"
    if [[ -n "$verified" ]] && git rev-parse -q --verify "$verified^{commit}" > /dev/null 2>&1; then
        # Older wins. If the two are unrelated, `--is-ancestor` says no and the branch point
        # stands, which is the wider answer.
        if git merge-base --is-ancestor "$verified" "$boundary" 2> /dev/null; then
            boundary="$verified"
        fi
    fi
    printf '%s\n' "$boundary"
}

# Every path the change touches: committed since $1, plus everything the working tree has,
# tracked or not. A rename shows as "old -> new"; both halves matter, so both are kept.
rd_scope_changed() {
    {
        git diff --name-only "$1" HEAD
        git status --porcelain --untracked-files=all | cut -c4- | sed 's/ -> /\n/'
    } | sed '/^$/d' | sort -u
}

# The crate directories under crates/ that the paths on stdin touch.
rd_scope_crate_dirs() {
    sed -n 's|^crates/\([^/]*\)/.*|\1|p' | sort -u
}

# The cargo package name of the crate directory $1, from its own manifest rather than from the
# directory name — the two agree today, and reading the manifest is what keeps that from being
# a silent assumption.
rd_scope_package_name() {
    local manifest="crates/$1/Cargo.toml"
    [[ -f "$manifest" ]] || return 0
    sed -n '/^\[package\]/,/^\[/p' "$manifest" | sed -n 's/^name = "\(.*\)"/\1/p' | head -1
}

# One level of reverse dependencies for the package names on stdin: every crate whose manifest
# mentions one of them. Deliberately a grep, as the plan specifies — a crate that names another
# in its manifest is a dependant, and the false positives (a name in a comment) only widen the
# run.
rd_scope_reverse_deps() {
    local names=() name manifest dependant
    mapfile -t names
    [[ ${#names[@]} -gt 0 ]] || return 0
    for name in "${names[@]}"; do
        [[ -n "$name" ]] || continue
        for manifest in crates/*/Cargo.toml; do
            grep -q "\b$name\b" "$manifest" || continue
            dependant="$(sed -n '/^\[package\]/,/^\[/p' "$manifest" | sed -n 's/^name = "\(.*\)"/\1/p' | head -1)"
            [[ -n "$dependant" ]] && printf '%s\n' "$dependant"
        done
    done
    printf '%s\n' "${names[@]}"
}

# --- Cargo.toml and Cargo.lock (RD-130-17) --------------------------------------------------
#
# The first path on stdin that governs the whole build and has no narrower answer: the
# toolchain, the nextest and the deny configuration. Nothing when there is none. The root
# Cargo.toml and Cargo.lock are not among them; rd_scope_lock_crates narrows those.
rd_scope_build_governing() {
    grep -E '^rust-toolchain\.toml$|^\.config/nextest\.toml$|^deny\.toml$' | head -1 || true
}

# What a change to the root Cargo.toml or Cargo.lock since $1 reaches: scripts/lib/lock-scope.py
# on the manifests at $1 against the working tree, whose header states the rules. Prints either
# `WIDE <reason>` or `<package> <dir> <why>` per affected member; fails when it cannot answer,
# which the caller must read as WIDE.
rd_scope_lock_crates() {
    local boundary="$1" old status=0 manifests=()
    old="$(mktemp -d)"
    mapfile -t manifests < <(git ls-tree -r --name-only "$boundary" | grep -E '(^|/)Cargo\.(toml|lock)$')
    if git archive --format=tar "$boundary" -- "${manifests[@]}" | tar -x -C "$old"; then
        python3 "$(dirname "${BASH_SOURCE[0]}")/lock-scope.py" "$old" . || status=$?
    else
        status=1
    fi
    rm -rf "$old"
    return "$status"
}

# --- crates the plugin components are built from --------------------------------------------
#
# The workspace crates a plugin links, directly or through another of them, one directory per
# line (`crates/rd-core/`). A change to any of them can change every component — in 1.2.2 a
# change to rd-plugin-api changed 72 of them and only the release pipeline noticed, twice — so
# the version check in lib/components.sh asks about every plugin when one is touched.
#
# Read from the path dependencies in the manifests rather than listed, so a crate is covered
# the day a plugin starts linking it. `[dev-dependencies]` are skipped: they never reach a
# component, and following them would pull in the host and half the workspace.
rd_path_dependencies() {
    awk '/^\[/ { dev = ($0 ~ /dev-dependencies/) }
         !dev && match($0, /path *= *"[^"]*"/) { print substr($0, RSTART, RLENGTH) }' "$@" \
        | sed 's/^path *= *"\(.*\)"$/\1/'
}

rd_plugin_linked_crates() {
    local queue=() seen=" " crate
    mapfile -t queue < <(rd_path_dependencies plugins/*/Cargo.toml \
        | sed -n 's|^\.\./\.\./crates/\([^/]*\).*|\1|p' | sort -u)
    while [[ ${#queue[@]} -gt 0 ]]; do
        crate="${queue[0]}"
        queue=("${queue[@]:1}")
        [[ "$seen" == *" $crate "* ]] && continue
        seen+="$crate "
        [[ -f "crates/$crate/Cargo.toml" ]] || continue
        mapfile -t -O "${#queue[@]}" queue < <(rd_path_dependencies "crates/$crate/Cargo.toml" \
            | sed -n 's|^\.\./\([^/]*\)$|\1|p')
    done
    for crate in $seen; do printf 'crates/%s/\n' "$crate"; done | LC_ALL=C sort
}

# --- deferral -------------------------------------------------------------------------------
#
# Which of the three deferrable classes a path belongs to, or `no` for anything else. The
# classes come straight from the job: text, translations, and appearance without behaviour.
#
#   $1  the boundary commit, for reading the pre-change version of a .vue/.css file
#   $2  the path
rd_defer_class() {
    local boundary="$1" path="$2"
    case "$path" in
        docs/*|*.md)              printf 'docs\n'; return 0 ;;
        web/src/locales/*)        printf 'locales\n'; return 0 ;;
        web/src/assets/*)         printf 'appearance\n'; return 0 ;;
        *.css)                    printf 'appearance\n'; return 0 ;;
        *.vue)
            if rd_vue_script_touched "$boundary" "$path"; then
                printf 'no\n'
            else
                printf 'appearance\n'
            fi
            return 0
            ;;
        *) printf 'no\n'; return 0 ;;
    esac
}

# The line ranges of the <script> blocks of the text on stdin, as "start end" pairs.
rd_script_ranges() {
    awk '
        /<script[ >]/ || /<script$/ { start = NR }
        /<\/script>/ { if (start) { print start, NR; start = 0 } }
    '
}

# Whether the change to the .vue file $2 since $1 touches a <script> block in either version.
#
# Conservative by construction: a file that cannot be read on the old side (a new file, a
# rename) counts as touched, because "I could not tell" must never read as "safe to defer".
rd_vue_script_touched() {
    local boundary="$1" path="$2" old new ranges hunk start count first last
    new="$(mktemp)"; old="$(mktemp)"
    # shellcheck disable=SC2064
    trap "rm -f '$new' '$old'" RETURN

    [[ -f "$path" ]] || return 0
    cat "$path" > "$new"
    git show "$boundary:$path" > "$old" 2> /dev/null || return 0

    # Hunk headers carry both sides: @@ -<old start>,<old count> +<new start>,<new count> @@
    while read -r hunk; do
        # old side
        start="${hunk%% *}"; start="${start#-}"
        count="${start#*,}"; first="${start%%,*}"
        [[ "$count" == "$first" ]] && count=1
        last=$(( first + count - 1 ))
        while read -r range_start range_end; do
            (( first <= range_end && last >= range_start )) && return 0
        done < <(rd_script_ranges < "$old")

        # new side
        start="${hunk##* }"; start="${start#+}"
        count="${start#*,}"; first="${start%%,*}"
        [[ "$count" == "$first" ]] && count=1
        last=$(( first + count - 1 ))
        while read -r range_start range_end; do
            (( first <= range_end && last >= range_start )) && return 0
        done < <(rd_script_ranges < "$new")
    done < <(diff -u "$old" "$new" | sed -n 's/^@@ \(-[0-9,]*\) \(+[0-9,]*\) @@.*/\1 \2/p')

    return 1
}

# --- rd-api integration binaries (RD-120-58) ------------------------------------------------
#
# At branch level only the binaries the change needs run; `--full` runs all of them. The
# mapping is the table in scripts/lib/rd-api-tests.map, whose header states the rules.

# The paths whose unmapped changes select every binary rather than none.
RD_API_TEST_DOMAIN='^crates/rd-api/|^crates/rd-core/|^crates/rd-db/migrations/'

# Every rd-api integration test binary, one per line, sorted bytewise.
rd_api_test_binaries() {
    local file
    for file in crates/rd-api/tests/*.rs; do
        [[ -f "$file" ]] && basename "$file" .rs
    done | LC_ALL=C sort
}

# The rows of the map file $1, comments and blank lines removed.
rd_api_test_rows() {
    grep -vE '^[[:space:]]*(#|$)' "$1"
}

# What each changed path on stdin demands, as `<binary> <path>` lines — `all` for every binary.
# Pure bash matching, no process per row: a change set of a few hundred paths against seventy
# rows would otherwise cost seconds in forks alone.
rd_api_test_demands() {
    local map="$1" path pattern names name matched row rows=()
    mapfile -t rows < <(rd_api_test_rows "$map")
    while read -r path; do
        [[ -n "$path" ]] || continue
        case "$path" in
            crates/rd-api/tests/common/*) printf 'all %s\n' "$path"; continue ;;
            crates/rd-api/tests/mcp/*) printf 'mcp %s\n' "$path"; continue ;;
            crates/rd-api/tests/*.rs)
                name="${path#crates/rd-api/tests/}"
                if [[ "$name" != */* ]]; then
                    # A deleted test file selects nothing: there is no binary left to run.
                    [[ -f "$path" ]] && printf '%s %s\n' "${name%.rs}" "$path"
                    continue
                fi
                ;;
        esac
        matched=0
        for row in "${rows[@]}"; do
            read -r pattern names <<< "$row"
            [[ "$path" =~ $pattern ]] || continue
            # `+` adds binaries without being a mapping; see the header of the map.
            [[ "$names" == "+ "* ]] || matched=1
            for name in $names; do
                [[ "$name" == - || "$name" == + ]] || printf '%s %s\n' "$name" "$path"
            done
        done
        if [[ "$matched" -eq 0 && "$path" =~ $RD_API_TEST_DOMAIN ]]; then
            printf 'all %s\n' "$path"
        fi
    done
}

# What is wrong with the map file $1, one finding per line; nothing when it is sound. A row
# naming a binary that does not exist would fail the run obscurely inside nextest, and a binary
# that no row names would only ever run at --full or by accident — so both are refused.
rd_api_test_map_problems() {
    local map="$1" binaries named
    binaries="$(rd_api_test_binaries)"
    named="$(rd_api_test_rows "$map" | while read -r _ names; do
        # shellcheck disable=SC2086 # the names are words by design
        printf '%s\n' $names
    done | grep -vxE 'all|-|\+' | LC_ALL=C sort -u)"
    LC_ALL=C comm -13 <(printf '%s\n' "$binaries") <(printf '%s\n' "$named") \
        | sed '/^$/d; s/^/a row names a binary that does not exist: /'
    LC_ALL=C comm -23 <(printf '%s\n' "$binaries") <(printf '%s\n' "$named") \
        | sed '/^$/d; s/^/no row names the binary: /'
}
