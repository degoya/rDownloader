#!/usr/bin/env bash
# shellcheck shell=bash
# shellcheck disable=SC2154  # the change flags are check.sh's, which sources this file
#
# The web and extension half of scripts/check.sh, kept here so check.sh stays readable
# (RD-1100-12 T16): the full typecheck, vitest and the build of web/ when it changed, the
# extension's build when extension/ did, each command through `attempt`.
#
# Expects from the caller: `run_web`, `web_changed`, `extension_changed`, `step`, `skip` and
# `attempt`, as check.sh defines them (`web_skip_reason` when lib/check-reuse.sh set one,
# `web_locales_only` when lib/check-reuse-crates.sh did), and the working directory at the checkout
# root.

rd_check_web() {
    # Only translation catalogues since a web green (RD-1150-06): Vitest, which loads every one.
    if [[ "$run_web" -eq 1 && "${web_locales_only:-0}" -eq 1 ]]; then
        local base="the web green of tree ${web_locales_base:0:12}"
        if [[ "${web_locales_typecheck:-0}" -eq 1 ]]; then
            step "pnpm run typecheck:full"
            attempt pnpm --dir web run typecheck:full
        else
            skip "pnpm run typecheck:full" "only translation catalogues changed since $base, and no source imports one by name"
        fi
        step "pnpm run test"
        attempt pnpm --dir web run test
        skip "pnpm run build" "only translation catalogues changed since $base; Vitest loaded every one, GitHub CI and the release build the frontend"
        skip "the browser extension" "$base covers it; nothing under extension/ changed since"
        return 0
    fi
    if [[ "$run_web" -eq 1 && "$web_changed" -eq 1 ]]; then
        # Non-incremental on every run, the one CI and the release chain run: the incremental
        # `typecheck` trusts web/tsconfig.*.tsbuildinfo, and two type errors it passed reached
        # GitHub on 2026-09-27 (RD-150-22).
        step "pnpm run typecheck:full"
        attempt pnpm --dir web run typecheck:full
        step "pnpm run test"
        attempt pnpm --dir web run test
        # Refused rather than run-and-warn. A feature worktree links web/dist to the main
        # checkout's (scripts/worktree.sh), and a build would empty and rewrite that one. Its own
        # web/node_modules (RD-150-14) is no longer a hazard: until 1.5 that was a link too, and
        # the unplugin generators wrote the other checkout's paths into the tracked declarations.
        #
        # Not built here, deliberately, also not by --full in an integration worktree (audit C7):
        # rd-api's `/` tests embed web/dist through rust-embed, which keeps serving the main
        # checkout's once compiled through the link (scripts/lib/web-dist.sh), so a build of this
        # branch's frontend would change nothing they see. The build of this content is GitHub's
        # (scripts/public-ci.sh, which integrate.sh --public-ci starts beside --full).
        if [[ -L web/dist ]]; then
            skip "pnpm run build" "a feature worktree: web/dist links the main checkout's, and rd-api's / tests ran against that frontend, not this branch's; GitHub CI builds this one (scripts/public-ci.sh)"
            echo
            echo "    pnpm run build is refused here: it would write into the main checkout's web/dist."
            echo "    'rm web/dist' (the link only) first to build in this worktree."
        elif scripts/web-dist-stale.sh > /dev/null; then
            skip "pnpm run build" "web/dist is newer than every source that goes into it"
        else
            step "pnpm run build"
            attempt pnpm --dir web run build
            # Belt and braces: these are generated and tracked, so a surprise diff is worth naming
            # even outside a worktree — it usually means the component inventory really did change
            # and the regenerated files belong in the commit.
            if ! git diff --quiet -- web/components.d.ts web/auto-imports.d.ts; then
                echo
                echo "!! web/components.d.ts or web/auto-imports.d.ts changed — review and commit them." >&2
            fi
        fi
    elif [[ "$run_web" -eq 1 ]]; then
        skip "typecheck, vitest and the web build" "nothing under web/ changed"
    fi

    if [[ "$run_web" -eq 1 && "$extension_changed" -eq 1 ]]; then
        step "browser extension"
        attempt scripts/build-extension.sh
    elif [[ "$run_web" -eq 1 ]]; then
        skip "the browser extension" "nothing under extension/ changed"
    fi
    if [[ "$run_web" -eq 0 ]]; then skip "the whole web half" "${web_skip_reason:---rust was given}"; fi
}
