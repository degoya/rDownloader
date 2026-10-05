#!/usr/bin/env bash
# shellcheck shell=bash
# shellcheck disable=SC2154  # `boundary` and `changed` are check.sh's, which sources this file
#
# The --defer run of scripts/check.sh, kept here so check.sh stays readable (RD-1100-12 T16):
# postpone a triviality, without letting it slip through. It refuses a change that is not one
# (rd_defer_class in lib/scope.sh), runs the seconds-long checks the classes it found ask for,
# moves no green record and ends the run.
#
# Expects from the caller: `step`, `boundary` and `changed`, as check.sh defines them, and the
# working directory at the checkout root.

rd_check_defer() {
    local path verdict
    local -a classes=()
    while read -r path; do
        [[ -n "$path" ]] || continue
        verdict="$(rd_defer_class "$boundary" "$path")"
        if [[ "$verdict" == "no" ]]; then
            echo >&2
            echo "!! --defer refused: $path is not a triviality." >&2
            echo "   Deferrable: docs/ and *.md, web/src/locales/**, web/src/assets/**," >&2
            echo "   and a .vue or .css change that does not touch a <script> block —" >&2
            echo "   never a file a Rust test reads (scripts/lib/rust-test-inputs.map)." >&2
            echo "   Run scripts/check.sh without --defer." >&2
            exit 1
        fi
        classes+=("$verdict")
    done <<< "$changed"

    step "git diff --check"
    git diff --check "$boundary"

    if printf '%s\n' "${classes[@]+"${classes[@]}"}" | grep -qx locales; then
        step "the four locale catalogues agree"
        pnpm --dir web test src/i18n/locales.test.ts
    fi
    if printf '%s\n' "${classes[@]+"${classes[@]}"}" | grep -qx appearance; then
        step "pnpm run typecheck:full"
        pnpm --dir web run typecheck:full
    fi

    echo
    echo "==> deferred, not verified"
    echo "    The green record was NOT moved forward. The next ordinary run measures its change"
    echo "    set against ${boundary:0:12} and therefore takes these commits with it."
    echo "    A deferred state is not 'tests pass': scripts/worktree.sh finish and"
    echo "    scripts/release-pipeline.sh refuse it."
    exit 0
}
