# shellcheck shell=bash
# shellcheck disable=SC2154  # the globals are release-pipeline.sh's, which sources this file
#
# The evidence machinery of scripts/release-pipeline.sh: the markers a step leaves in the
# evidence log, and run_step / run_steps_parallel, which run a step and write its marker. The
# properties it keeps are named at the top of release-pipeline.sh.
#
# Expects from scripts/release-pipeline.sh, which sources it: ROOT, VERSION, LOG, NONCE, RESUME,
# PRERELEASE, RELEASE_BRANCH, MAIN_BRANCH, JOBS, LANES, WINDOWS_LANE, STEP_IDS, GATE_REQUIRES and
# step_command, rd_worktree_tree and rd_paths_read_by (scripts/lib/verified.sh and the
# inert-paths.sh it sources), and the working directory at the checkout root.

marker_field() { sed -n "s/.* $2=\([^ ]*\).*/\1/p" <<< "$1"; }

# The last line recorded for a step, for this run only and, but for preflight, since the tree last
# changed under it (resume_tree_check). Empty when there is none.
last_marker_line() {
    awk -v id="id=$1" -v nonce="nonce=$NONCE" '
        $1 == "##RD-TREE-CHANGED" && $2 == nonce && id != "id=preflight" { marker = "" }
        $1 == "##RD-STEP" && $2 == id && $3 == nonce { marker = $0 }
        END { if (marker != "") print marker }' "$LOG" 2> /dev/null || true
}

# The last recorded attempt at a step (last_marker_line). A green carried across a change of the
# tree (`reused=`, resume_tree_check) counts only while the diff between the tree it was recorded
# for and the tree it was carried to touches nothing the step reads (RD-1270-12) — checked again
# on every read, so the evidence gate holds a carried record to the same rule. Empty when there is
# none or the carry does not hold.
last_marker() {
    local marker from touched
    marker="$(last_marker_line "$1")"
    [[ -n "$marker" ]] || return 0
    from="$(marker_field "$marker" reused)"
    if [[ -n "$from" ]]; then
        touched="$(step_touched "$1" "$from" "$(marker_field "$marker" tree)")" || return 0
        [[ -z "$touched" ]] || return 0
    fi
    printf '%s\n' "$marker"
}

# The tree of the working state a step leaves behind, for its marker (PIPE-09); `none` when git
# cannot say, which no later tree equals.
step_tree() {
    local tree; tree="$(rd_worktree_tree "$ROOT" || true)"
    printf '%s\n' "${tree:-none}"
}

# What each step reads (RD-1270-12): of the changed paths on stdin, those step $1 reads, by the
# path classes of rd_paths_read_by (scripts/lib/inert-paths.sh).
#   test                        the Rust inputs and what the preflight reads: `check.sh --rust
#                               --full` runs the preflight's checks of the content as well
#   clippy                      the Rust inputs
#   web                         web/, extension/ and scripts/build-extension.sh
#   sign-plugins                the Rust inputs: the plugins, the WIT, the crates the components
#                               link (rd-plugin-api, rd-plugin-types) and the signer rd-pack is
#                               built from, the manifests, lock file and toolchain the component
#                               cache key reads, build-plugins.sh and its stamp library
#   build-linux, build-windows  the Rust inputs, the web half without its tests, what the package
#                               carries (README.md, LICENSE, resources/, its launchers) and the
#                               scripts that put it together
#   every other step            every path, documentation included: the version bump, the
#                               documentation steps, the commit and the merge read the whole
#                               tree; compat, verify-artifacts and smoke take seconds
RD_STEP_WEB_TESTS_PATTERN='^web/e2e/|^extension/test/|\.(test|spec)\.[cm]?[jt]s$|^web/vitest\.config\.ts$'
RD_STEP_PACKAGE_PATTERN='^README\.md$|^LICENSE$|^resources/|^scripts/web-dist-stale\.sh$|^scripts/lib/(package|version-file|web-dist|archive-layout|workspace-version)\.sh$'
step_reads() {
    local all
    all="$(sed '/^$/d')"
    [[ -n "$all" ]] || return 0
    case "$1" in
        test)
            { rd_paths_read_by rust <<< "$all"; rd_paths_read_by preflight <<< "$all"; } \
                | sort -u ;;
        clippy|web) rd_paths_read_by "$1" <<< "$all" ;;
        sign-plugins) rd_paths_read_by rust <<< "$all" ;;
        build-linux|build-windows)
            local platform="${1#build-}"
            { rd_paths_read_by rust <<< "$all"
              rd_paths_read_by web <<< "$all" | { grep -vE "$RD_STEP_WEB_TESTS_PATTERN" || true; }
              grep -E "$RD_STEP_PACKAGE_PATTERN|^scripts/package-$platform\.sh$|^scripts/$platform/" \
                  <<< "$all" || true
            } | sort -u ;;
        *) printf '%s\n' "$all" ;;
    esac
}

# The paths step $1 reads that changed between tree $2 and tree $3, none when it reads nothing of
# the diff. Fails when git cannot compare the two (`none`, an empty field, an object it lacks).
step_touched() {
    local diff
    [[ -n "$2" && -n "$3" && "$2" != none && "$3" != none ]] || return 1
    diff="$(git -C "$ROOT" diff-tree -r --no-renames --name-only "$2" "$3" 2> /dev/null)" || return 1
    step_reads "$1" <<< "$diff"
}

# On --resume (PIPE-09): the working tree is compared with the one this run's last step left
# behind. The same tree means nothing changed between the stop and the resume, and every green
# step stands. Any other — a fix between the stop and the resume — writes a ##RD-TREE-CHANGED line,
# behind which last_marker sees no earlier record. A green step whose inputs (step_reads) the diff
# between the tree its marker recorded and the tree found does not touch is carried across it
# (RD-1270-12): a marker behind the change line repeats the green's exit, bytes, start and end,
# names the tree found as `tree=` and the tree the green was recorded for as `reused=`, and
# last_marker checks that diff again whenever it reads one. Every other step runs again. The diff
# runs from the step's own tree, so it includes what the chain changed after the step (doc-facts,
# archive-jobs): the packages carry README.md, and a step that reads everything stands only on its
# own tree. preflight stands without a carry: it is the start gate and wants a committed tree,
# which the version bump no longer is, and its checks of the content run again inside test's
# `check.sh --rust --full`, which reads every path the preflight reads.
resume_tree_check() {
    local recorded current id marker from touched count
    local -a carried=() notes=()
    recorded="$(awk -v nonce="nonce=$NONCE" '
        ($1 == "##RD-STEP" && $3 == nonce) || ($1 == "##RD-TREE-CHANGED" && $2 == nonce) { line = $0 }
        END { print line }' "$LOG" 2> /dev/null || true)"
    [[ -n "$recorded" ]] || return 0
    recorded="$(marker_field "$recorded" tree)"
    current="$(step_tree)"
    if [[ -n "$recorded" && "$recorded" == "$current" && "$current" != none ]]; then
        echo "==> the tree is the one the last step left (${current:0:12}); its green steps stand"
        return 0
    fi
    # Decided before the change line is written, behind which no earlier marker is seen.
    for id in "${STEP_IDS[@]}"; do
        [[ "$id" != preflight ]] || continue
        step_is_green "$id" || continue
        marker="$(last_marker "$id")"
        from="$(marker_field "$marker" reused)"
        [[ -n "$from" ]] || from="$(marker_field "$marker" tree)"
        if ! touched="$(step_touched "$id" "$from" "$current")"; then
            notes+=("[$id] runs again: its marker names no tree git can compare")
        elif [[ -n "$touched" ]]; then
            count="$(wc -l <<< "$touched")"
            notes+=("[$id] runs again: the change touches ${touched%%$'\n'*}$(
                [[ "$count" -le 1 ]] || echo " and $((count - 1)) more")")
        else
            carried+=("$(printf '##RD-STEP id=%s nonce=%s version=%s exit=0 bytes=%s started=%s ended=%s tree=%s reused=%s' \
                "$id" "$NONCE" "$VERSION" "$(marker_field "$marker" bytes)" \
                "$(marker_field "$marker" started)" "$(marker_field "$marker" ended)" "$current" "$from")")
            notes+=("[$id] stands: the change touches nothing it reads; green of tree ${from:0:12} ($(marker_field "$marker" started))")
        fi
    done
    {
        printf '##RD-TREE-CHANGED nonce=%s was=%s tree=%s at=%s\n' \
            "$NONCE" "${recorded:-none}" "$current" "$(date -Is)"
        [[ ${#carried[@]} -eq 0 ]] || printf '%s\n' "${carried[@]}"
    } >> "$LOG"
    echo "==> the tree changed since the last step (${recorded:-none} -> ${current:0:12}):"
    echo "    preflight stands; every other step runs again unless the change misses what it reads"
    [[ ${#notes[@]} -eq 0 ]] || printf '    %s\n' "${notes[@]}"
}

step_is_green() {
    local marker; marker="$(last_marker "$1")"
    [[ -n "$marker" ]] || return 1
    [[ "$(marker_field "$marker" exit)" == "0" ]] || return 1
    [[ "$(marker_field "$marker" bytes)" -gt 0 ]] || return 1
    return 0
}

run_step() {
    local id="$1"; shift
    local -a codes
    local started ended before after status bytes

    if [[ "$RESUME" -eq 1 ]] && step_is_green "$id"; then
        echo "==> [$id] already green in this run — skipped"
        return 0
    fi

    printf '\n===== STEP %s (%s) =====\n' "$id" "$(date -Is)" >> "$LOG"
    started="$(date -Is)"
    before="$(stat -c %s "$LOG")"

    echo
    echo "==> [$id]"
    # The exit status comes from PIPESTATUS[0]. `set +e` is what lets us read it at all: with
    # `set -e` still armed the script would leave before the assignment. Nothing is filtered
    # between the command and tee, so nothing can turn a failure into a success.
    set +e
    "$@" 2>&1 | tee -a "$LOG"
    codes=("${PIPESTATUS[@]}")
    set -e
    status="${codes[0]}"

    after="$(stat -c %s "$LOG")"
    bytes="$(( after - before ))"
    ended="$(date -Is)"
    printf '##RD-STEP id=%s nonce=%s version=%s exit=%s bytes=%s started=%s ended=%s tree=%s\n' \
        "$id" "$NONCE" "$VERSION" "$status" "$bytes" "$started" "$ended" "$(step_tree)" >> "$LOG"

    if [[ "$status" -ne 0 ]]; then
        echo >&2
        echo "!! [$id] failed with exit $status — the pipeline stops here." >&2
        echo "   evidence so far: $LOG" >&2
        echo "   fix it, then: scripts/release-pipeline.sh $VERSION --resume" >&2
        exit "$status"
    fi
    if [[ "$bytes" -le 0 ]]; then
        echo >&2
        echo "!! [$id] exited 0 but produced no output; that is missing evidence, not a pass." >&2
        exit 1
    fi
}

# Several steps at once, each in its own lane (RD-140-06). Every step writes a part log of its
# own; once all have ended, the evidence log gets each part in turn under the header and the
# marker run_step writes, with the step's own start and end, so the gate reads a parallel run
# exactly like a serial one. The exit status is the step function's own, written by the subshell
# that ran it — a part without that file counts as failed. Output is not shown live, since two
# builds interleaved line by line help nobody; the part logs are named for `tail -f`.
run_steps_parallel() {
    local -a ids=() pids=()
    local id part pid status started ended bytes tree failed=0 first_status=0
    for id in "$@"; do
        if [[ "$RESUME" -eq 1 ]] && step_is_green "$id"; then
            echo "==> [$id] already green in this run — skipped"
            continue
        fi
        ids+=("$id")
    done
    if [[ ${#ids[@]} -lt 2 ]]; then
        for id in "${ids[@]}"; do run_step "$id" "$(step_command "$id")"; done
        return 0
    fi

    echo
    echo "==> [${ids[*]}] in parallel, one lane each; output while they run:"
    for id in "${ids[@]}"; do
        part="$LOG.$id.part"
        rm -f "$part" "$part.status"
        echo "    tail -f $part"
        (
            set +e
            started="$(date -Is)"
            "$(step_command "$id")" > "$part" 2>&1
            status=$?
            printf '%s %s %s\n' "$status" "$started" "$(date -Is)" > "$part.status"
        ) &
        pids+=("$!")
    done
    for pid in "${pids[@]}"; do wait "$pid" || true; done
    tree="$(step_tree)"

    for id in "${ids[@]}"; do
        part="$LOG.$id.part"
        status=1 started="?" ended="?"
        read -r status started ended 2> /dev/null < "$part.status" \
            || echo "!! [$id] left no exit status; counted as failed" >&2
        printf '\n===== STEP %s (%s) =====\n' "$id" "$started" >> "$LOG"
        cat "$part" >> "$LOG" 2> /dev/null || true
        bytes="$(stat -c %s "$part" 2> /dev/null || echo 0)"
        printf '##RD-STEP id=%s nonce=%s version=%s exit=%s bytes=%s started=%s ended=%s tree=%s\n' \
            "$id" "$NONCE" "$VERSION" "$status" "$bytes" "$started" "$ended" "$tree" >> "$LOG"
        rm -f "$part" "$part.status"
        echo "==> [$id] exit $status, $bytes bytes of output ($started → $ended)"
        if [[ "$status" -ne 0 ]]; then
            echo "!! [$id] failed with exit $status" >&2
            failed=1
            [[ "$first_status" -ne 0 ]] || first_status="$status"
        elif [[ "$bytes" -le 0 ]]; then
            echo "!! [$id] exited 0 but produced no output; that is missing evidence, not a pass." >&2
            failed=1
            [[ "$first_status" -ne 0 ]] || first_status=1
        fi
    done
    if [[ "$failed" -ne 0 ]]; then
        echo "   the pipeline stops here; evidence so far: $LOG" >&2
        echo "   fix it, then: scripts/release-pipeline.sh $VERSION --resume" >&2
        exit "$first_status"
    fi
}
