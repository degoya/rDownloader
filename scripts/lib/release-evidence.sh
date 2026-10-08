# shellcheck shell=bash
# shellcheck disable=SC2154  # the globals are release-pipeline.sh's, which sources this file
#
# The evidence machinery of scripts/release-pipeline.sh: the markers a step leaves in the
# evidence log, and run_step / run_steps_parallel, which run a step and write its marker. The
# properties it keeps are named at the top of release-pipeline.sh.
#
# Expects from scripts/release-pipeline.sh, which sources it: ROOT, VERSION, LOG, NONCE, RESUME,
# PRERELEASE, RELEASE_BRANCH, MAIN_BRANCH, JOBS, LANES, WINDOWS_LANE, GATE_REQUIRES and
# step_command, rd_worktree_tree (scripts/lib/verified.sh), and the working directory at the
# checkout root.

marker_field() { sed -n "s/.* $2=\([^ ]*\).*/\1/p" <<< "$1"; }

# The last recorded attempt at a step, for this run only and, but for preflight, since the tree
# last changed under it (resume_tree_check). Empty when there is none.
last_marker() {
    awk -v id="id=$1" -v nonce="nonce=$NONCE" '
        $1 == "##RD-TREE-CHANGED" && $2 == nonce && id != "id=preflight" { marker = "" }
        $1 == "##RD-STEP" && $2 == id && $3 == nonce { marker = $0 }
        END { if (marker != "") print marker }' "$LOG" 2> /dev/null || true
}

# The tree of the working state a step leaves behind, for its marker (PIPE-09); `none` when git
# cannot say, which no later tree equals.
step_tree() {
    local tree; tree="$(rd_worktree_tree "$ROOT" || true)"
    printf '%s\n' "${tree:-none}"
}

# On --resume (PIPE-09): the working tree is compared with the one this run's last step left
# behind. The same tree means nothing changed between the stop and the resume, and every green
# step stands. Any other — a fix between the stop and the resume — writes a ##RD-TREE-CHANGED line,
# behind which last_marker sees no record: every step after preflight runs again, sign-plugins and
# the packages among them, and the evidence gate wants the new records. Per step it cannot be
# decided: the chain changes the tree itself (version-bump, doc-facts, archive-jobs), and the
# packages carry README.md, so even a documentation fix leaves them stale. preflight stands: it is
# the start gate and wants a committed tree, which the version bump no longer is, and its checks
# of the content run again inside test's `check.sh --rust --full`.
resume_tree_check() {
    local recorded current
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
    printf '##RD-TREE-CHANGED nonce=%s was=%s tree=%s at=%s\n' \
        "$NONCE" "${recorded:-none}" "$current" "$(date -Is)" >> "$LOG"
    echo "==> the tree changed since the last step (${recorded:-none} -> ${current:0:12}):"
    echo "    every step after preflight runs again"
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
