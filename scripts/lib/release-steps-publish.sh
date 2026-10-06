# shellcheck shell=bash
# shellcheck disable=SC2154  # the globals are release-pipeline.sh's, which sources this file
#
# The release chain's steps from doc-facts to publish-public: the documentation gate, the
# release commit, the merge into main, the evidence gate, the tag, the push and the public
# export, and the trap that leaves the checkout on the release branch
# (scripts/release-pipeline.sh).
#
# Expects from scripts/release-pipeline.sh, which sources it: VERSION, LOG, NONCE, RESUME,
# PRERELEASE, RELEASE_BRANCH, MAIN_BRANCH, JOBS, LANES, WINDOWS_LANE, GATE_REQUIRES and
# step_command, and the working directory at the checkout root.

# The facts the documentation repeats and a release moves — the version and its date in the
# feature list, the plugin count, the contract `rdownloader:plugin@X.Y.Z` — are mechanics, not
# judgement, so they are written from their sources here rather than remembered by hand
# (RD-140-24). commit-guard's `git add -A` takes the result into the release commit. The user wiki
# is another repository and stays the wiki pass's to write; docs-gate only checks it.
step_doc_facts() { scripts/doc-facts.sh; }

# Not a generator — a gate. A changelog entry is judgement, and a pipeline that writes its own
# release notes is a pipeline that certifies its own work. This only refuses to continue when
# the documents AGENTS.md names have not been brought up to date by hand.
step_docs_gate() {
    local failed=0

    if ! grep -q "^## \[$VERSION\]" CHANGELOG.md; then
        echo "CHANGELOG.md has no '## [$VERSION]' section" >&2; failed=1
    else
        echo "CHANGELOG.md: $(grep -c . <<< "$(awk -v v="## [$VERSION]" '
            $0 ~ "^## \\[" { inside = (index($0, v) == 1) } inside' CHANGELOG.md)") lines for $VERSION"
    fi

    # The version has moved; anything still naming the previous one is stale by definition.
    #
    # The last *shipped* release is the honest reference: the highest `vX.Y.Z` tag under the
    # version being released (scripts/lib/release-tag.sh). Neither HEAD's Cargo.toml — bumped by
    # hand beforehand, as a release prepared over several sessions is, it names the version being
    # released, and on 1.0.9 the gate called 860 new changelog lines untouched — nor `git
    # describe`: release tags sit on main's merge commits, so on development it answered a
    # release several versions old, whose changelog diff anything passes.
    local previous
    previous="$(rd_release_tag_at_most "$VERSION" --below)"
    previous="${previous#v}"
    echo "previous version: ${previous:-none}"

    # What this has to establish is that the changelog was written for *this* release, and
    # the honest comparison is against the last one that shipped. Asking whether the file is
    # dirty right now cannot work: preflight refuses to start from an uncommitted tree, and
    # nothing between the two steps touches CHANGELOG.md, so the two gates contradicted each
    # other and the chain could never reach its tag. The working-tree check stays as the
    # fallback for the first release, when there is no previous tag to compare against.
    if git rev-parse -q --verify "refs/tags/v$previous" >/dev/null 2>&1; then
        if git diff --quiet "v$previous" -- CHANGELOG.md; then
            echo "CHANGELOG.md is unchanged since v$previous" >&2; failed=1
        else
            echo "CHANGELOG.md: changed since v$previous"
        fi
    elif ! git diff --quiet -- CHANGELOG.md; then
        echo "CHANGELOG.md: modified in this release"
    else
        echo "CHANGELOG.md was not touched for $VERSION" >&2; failed=1
    fi

    local doc
    for doc in README.md docs/roadmap.md; do
        [[ -f "$doc" ]] || { echo "missing $doc" >&2; failed=1; continue; }
        echo "$doc: present, $(wc -l < "$doc") lines"
    done

    # A roadmap that still calls this milestone planned, after the milestone shipped, is the
    # drift AGENTS.md forbids. Named rather than auto-corrected.
    if grep -rn "$VERSION" docs/roadmap.md > /dev/null 2>&1; then
        echo "docs/roadmap.md mentions $VERSION"
    else
        echo "docs/roadmap.md never mentions $VERSION" >&2; failed=1
    fi

    # doc-facts wrote them; this proves nothing edited them back, and holds the user wiki to the
    # same facts. The wiki is updated at the tag from this release's section, before the chain
    # publishes it (publish-public), so a stale contract or count there stops the release here.
    # The same holds for the contract reference the plugin reference carries (RD-160-04): a WIT
    # change without `scripts/wit-reference.sh --wiki` in the wiki pass stops here too.
    local wiki="${RD_WIKI_SRC:-$HOME/projects/rdownloader.wiki}"
    if [[ -d "$wiki" ]]; then
        scripts/doc-facts.sh --check --wiki "$wiki" || failed=1
        scripts/wit-reference.sh --wiki "$wiki" --check || failed=1
    else
        echo "no user wiki at $wiki; the facts are checked in this repository only"
        scripts/doc-facts.sh --check || failed=1
        scripts/wit-reference.sh --check || failed=1
    fi

    [[ "$failed" -eq 0 ]]
}

# The job files this release finished move into docs/roadmap/jobs/archive/, with their links and
# index rows (RD-140-19). After docs-gate, so the hand-written status changes are in; before
# commit-guard, whose `git add -A` takes the moves into the release commit. --release makes the
# working file of this release count as tagged, because the tag comes later. With nothing due it
# prints so and passes. A resumed run skips it once green; run again it would find nothing due.
step_archive_jobs() { scripts/archive-jobs.sh --release "$VERSION"; }

# Requirement: nothing linked, nothing generated, nothing built may enter the release commit.
# Runs against the staged index, before the commit exists and long before the merge.
step_commit_guard() {
    git add -A
    local failed=0

    echo "--- staged ---"
    git diff --cached --name-status

    local links
    links="$(git diff --cached --raw | awk '$2 == "120000" || $1 == ":120000" { print $NF }')"
    if [[ -n "$links" ]]; then
        echo "symlinks staged:" >&2; echo "$links" >&2; failed=1
    else
        echo "symlinks: none"
    fi

    # web/dist and node_modules are gitignored twice over — with and without the trailing slash,
    # because a worktree makes them symlinks and a directory pattern would miss those. This is
    # the belt to that braces: even if the ignore rules were edited away, the commit is refused.
    local forbidden
    # dist/plugins/.keep is tracked on purpose — it is the placeholder that makes the directory
    # exist, not build output — so it is the one path this pattern must not catch.
    forbidden="$(git diff --cached --name-only \
        | grep -E '(^|/)node_modules(/|$)|(^|/)dist(/|$)|^artifacts/|^target/|\.rdplug$' \
        | grep -vxF 'dist/plugins/.keep' || true)"
    if [[ -n "$forbidden" ]]; then
        echo "build output or dependencies staged:" >&2; echo "$forbidden" >&2; failed=1
    else
        echo "build output: none staged"
    fi

    # An empty index is only a defect when the release content is still missing. A release
    # prepared over several sessions has the version bump, the changelog section and the roadmap
    # status committed long before the pipeline runs, and then there is nothing left to commit —
    # which is a finished state, not an unstarted one. Measured on 1.0.9, where every one of the
    # three was already on `development`.
    if [[ -z "$(git diff --cached --name-only)" ]]; then
        if [[ "$(scripts/set-version.sh)" == "$VERSION" ]] && grep -q "^## \[$VERSION\]" CHANGELOG.md; then
            echo "nothing staged: the version bump and the changelog are already committed"
        else
            echo "nothing staged, and the release content is not committed either" >&2; failed=1
        fi
    fi

    [[ "$failed" -eq 0 ]]
}

step_commit() {
    if [[ -z "$(git diff --cached --name-only)" ]]; then
        echo "nothing to commit; the release content is already on this branch"
        git --no-pager show --stat --oneline HEAD
        return 0
    fi
    git commit -m "chore(release): $VERSION"
    git --no-pager show --stat --oneline HEAD
}

step_merge_main() {
    local from; from="$(git rev-parse --abbrev-ref HEAD)"
    echo "merging $from into $MAIN_BRANCH"
    git checkout "$MAIN_BRANCH"
    if ! git merge --no-ff "$from" -m "Merge branch '$from' — release $VERSION"; then
        echo "the merge conflicted; leaving $MAIN_BRANCH untouched" >&2
        git merge --abort || true
        git checkout "$from"
        return 1
    fi
    git --no-pager log --oneline -1
    # The guard again, on what the merge actually produced.
    local links
    links="$(git ls-tree -r HEAD | awk '$1 == "120000" { print $4 }')"
    if [[ -n "$links" ]]; then
        echo "the merged tree contains symlinks:" >&2; echo "$links" >&2; return 1
    fi
    echo "merged tree: no symlinks"
}

step_evidence_gate() {
    local id marker exit_code bytes failed=0
    echo "verifying evidence for run $NONCE in $LOG"
    if ! grep -q "^##RD-RELEASE version=$VERSION nonce=$NONCE " "$LOG"; then
        echo "the log has no header for this run" >&2; return 1
    fi
    for id in "${GATE_REQUIRES[@]}"; do
        marker="$(last_marker "$id")"
        if [[ -z "$marker" ]]; then
            printf '  %-18s NO EVIDENCE\n' "$id" >&2; failed=1; continue
        fi
        exit_code="$(marker_field "$marker" exit)"
        bytes="$(marker_field "$marker" bytes)"
        if [[ "$exit_code" != "0" ]]; then
            printf '  %-18s exit=%s\n' "$id" "$exit_code" >&2; failed=1
        elif [[ "${bytes:-0}" -le 0 ]]; then
            printf '  %-18s exit=0 but 0 bytes of output\n' "$id" >&2; failed=1
        else
            printf '  %-18s exit=0, %s bytes\n' "$id" "$bytes"
        fi
    done
    if [[ "$failed" -ne 0 ]]; then
        echo "refusing to tag: the evidence is incomplete" >&2
        return 1
    fi
    echo "all ${#GATE_REQUIRES[@]} steps have passing, non-empty evidence"
}

step_tag() {
    scripts/tag-release.sh "$VERSION"
    # `head` closes the pipe as soon as it has its lines, git dies of SIGPIPE, and PIPESTATUS[0]
    # reports 141 — so the step failed on its own summary while the tag it exists to create was
    # already made by the line above. `sed` reads the whole stream instead of walking away.
    git --no-pager show --stat --oneline "v$VERSION" | sed -n '1,20p'
}

# A pre-release pushes no main: merge-main did not run, and main keeps the last stable release.
step_push() {
    if [[ "$PRERELEASE" -eq 0 ]]; then
        git push origin "$MAIN_BRANCH" || return 1
    fi
    git push origin "$RELEASE_BRANCH" || return 1
    git push origin "v$VERSION" || return 1
    echo "pushed $([[ $PRERELEASE -eq 0 ]] && echo "$MAIN_BRANCH, ")$RELEASE_BRANCH and v$VERSION to origin"
}

# The public CI on the candidate, before the tag (RD-130-23). GitHub's free runners check Linux,
# Windows and macOS, which this machine cannot; so the merged candidate goes to the public
# repository as the branch ci/<version>, and the tag is only made once its CI run is green.
#
# The trade-off, deliberately accepted: while the branch exists the candidate is public before
# it is a release. On green the branch is deleted at once. On red it stays, so the failed run
# can be read next to its tree, and the next run removes it — the same version's branch is
# replaced by the force push, any other ci/* branch is deleted before the new one is watched.
# The export, the wait and the deletion are scripts/lib/public-ci.sh, which scripts/public-ci.sh
# shares for integration branches (RD-140-22). Outward, so only with --push, like `push` itself.
#
# Only the platforms not yet green (RD-160-06): the candidate is its wave's integration tree plus
# the version bump and the release's documentation, and that integration branch passed Linux and
# Windows before it reached development — so as a rule macOS alone is dispatched, without the
# once-per-run jobs those runs passed (RD-1120-07), and with every platform green on record ci.yml
# is not dispatched at all. What it relies on is named in the log.
#
# Beside ci.yml the release workflows — E2E, Recovery, Self-update, Installers — run on the same
# branch, each not yet green for this content (owner, 2026-10-06, RD-1120-07): they ran on the
# push to `main` after the tag before, where a red one could hold nothing. They run beside the
# macOS run, not after it; one red run holds the tag.
step_public_ci() {
    local branch="ci/$VERSION" sha tree platforms="" workflow
    local -a images workflows
    # run_step calls a step without errexit, so every command that matters is checked here.
    tree="$(git rev-parse 'HEAD^{tree}')" || return 1
    echo "the release workflows for tree ${tree:0:12}:"
    rd_public_ci_plan "$ROOT" "$tree" "${RD_PUBLIC_CI_RELEASE_WORKFLOWS[@]}"
    workflows=(${RD_PUBLIC_CI_MISSING[@]+"${RD_PUBLIC_CI_MISSING[@]}"})
    mapfile -t images < <(rd_public_ci_images "$RD_PUBLIC_CI_ALL")
    echo "the public CI for tree ${tree:0:12}:"
    rd_public_ci_plan "$ROOT" "$tree" "${images[@]}"
    if [[ ${#RD_PUBLIC_CI_MISSING[@]} -eq 0 && ${#workflows[@]} -eq 0 ]]; then
        echo "every platform and release workflow is green on record for this content; the public CI is not run again"
        return 0
    fi
    if [[ ${#RD_PUBLIC_CI_MISSING[@]} -gt 0 ]]; then
        platforms="$(rd_public_ci_platforms "$(IFS=,; echo "${RD_PUBLIC_CI_MISSING[*]}")")" || return 1
    fi
    rd_public_ci_gh_ready || return 1
    scripts/export-public.sh "$VERSION" --ref HEAD --branch "$branch" --skip-push-ci || return 1
    sha="$(git -C "$PUBLIC_DIR" rev-parse "refs/heads/$branch")" || return 1
    rd_public_ci_prune_stale "$branch"
    if [[ -n "$platforms" ]]; then
        rd_public_ci_dispatch "$branch" "$platforms" "$(rd_public_ci_once_jobs "$ROOT" "$tree")" || return 1
    fi
    for workflow in ${workflows[@]+"${workflows[@]}"}; do
        rd_public_ci_dispatch_workflow "$branch" "$workflow" || return 1
    done
    rd_public_ci_wait "$branch" "$sha" workflow_dispatch \
        || { echo "the tag is not made while the public CI is not green" >&2; return 1; }
    rd_record_ci "$ROOT" "$tree" ${RD_PUBLIC_CI_MISSING[@]+"${RD_PUBLIC_CI_MISSING[@]}"} \
        ${workflows[@]+"${workflows[@]}"}
    rd_public_ci_delete "$branch"
}

# The public repository gets the tag as one fresh commit (RD-130-23). It runs after `push` so
# that nothing reaches the public side before the private one has it, and it publishes only
# when this run was asked to push; otherwise the export waits committed in the local clone.
# The user handbook follows into the repository's GitHub wiki, and the website's release facts
# follow both, all under the same --push rule. The website is never deployed here: the owner
# uploads the directory update-website.sh names by hand.
#
# A pre-release is exported alone: the handbook and the website describe the stable release, and
# both scripts refuse a beta, so they wait for it.
step_publish_public() {
    local push=()
    [[ "$DO_PUSH" -eq 0 ]] || push=(--push)
    scripts/export-public.sh "$VERSION" "${push[@]}" || return 1
    if [[ "$PRERELEASE" -eq 1 ]]; then
        echo "pre-release: the public wiki and the website stay on the last stable release"
        return 0
    fi
    scripts/export-wiki.sh "$VERSION" "${push[@]}" || return 1
    scripts/update-website.sh "$VERSION" "${push[@]}"
}

# The checkout ends on the release branch (RD-160-06). merge-main checks out main, and
# evidence-gate, public-ci and tag need it there — the tag is made at HEAD — but a chain that ended
# on main left the next commit there: on 2026-09-28 two commits landed on main by accident. From
# merge-main on, an exit trap switches back, after the last step as after a failure, and says so;
# a resumed run goes back onto main before continuing past merge-main.
return_to_release_branch() {
    [[ "$(git rev-parse --abbrev-ref HEAD 2> /dev/null)" == "$MAIN_BRANCH" ]] || return 0
    if git checkout -q "$RELEASE_BRANCH"; then
        echo "==> the checkout is back on $RELEASE_BRANCH; $MAIN_BRANCH is merged into, never committed on"
    else
        echo "!! could not switch back to $RELEASE_BRANCH: this checkout is still on $MAIN_BRANCH." >&2
        echo "   git checkout $RELEASE_BRANCH before committing anything." >&2
    fi
}
