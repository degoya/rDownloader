# shellcheck shell=bash
#
# Whether a recorded preflight green stands for the content a run is about to check (RD-1270-13),
# beside scripts/lib/verified.sh, whose records it reads. The release ran the preflight three times
# on nearly one content: release-start.sh's --full, the pipeline's preflight step, and the test
# step's --rust --full after the version bump — about 4.5 minutes per release.
#
# Two answers, for two questions:
#
#   * rd_preflight_green — for `check.sh --preflight`: a green of this very tree, or of HEAD's
#     tree while the working tree differs from HEAD by a version bump only (rd_prebump_green, the
#     rule the release's test, clippy and web steps apply). The run then checks only the file
#     checks, which read the environment (the job layout) and the version copies, and ends.
#   * rd_preflight_covered — for a --full's script stages (lib/check-reuse.sh): what
#     rd_full_covering says, the generators' output excepted, or the same version-bump carry.
#
# Expects verified.sh to be sourced.

# The tree of the preflight green that stands for tree $2 of checkout $1; nothing when none does.
rd_preflight_green() {
    local root="$1" tree="$2"
    [[ -n "$tree" ]] || return 0
    if rd_full_recorded "$root" preflight | grep -qxF "$tree"; then
        printf '%s\n' "$tree"
        return 0
    fi
    rd_prebump_green "$root" preflight
}

# Whether a preflight green covers tree $2 of checkout $1 for a --full's script stages.
rd_preflight_covered() {
    [[ -n "$(rd_full_covering "$1" preflight "$2")" || -n "$(rd_prebump_green "$1" preflight)" ]]
}
