# shellcheck shell=bash
#
# The web/dist trap of a feature worktree (RD-1120-06, audit P10/A4), refused before it falsifies
# a run.
#
# crates/rd-api/src/static_assets.rs embeds `../../web/dist` through rust-embed. In a debug build
# rust-embed reads the files at run time from the folder it canonicalised at compile time —
# through a worktree's link (scripts/worktree.sh) that is the MAIN checkout's web/dist — and names
# none of them in the dep-info, so cargo does not rebuild when the link becomes a real directory,
# and sccache hands back the old object either way. A worktree that builds in the main checkout's
# target/ with a web/dist of its own therefore tests, packages or drives the main checkout's
# frontend while it looks like its own.
#
# Refused rather than repaired. The two repairs the job weighed — remembering the realpath of
# web/dist beside the target and touching static_assets.rs with SCCACHE_RECACHE on a change, or a
# build.rs in rd-api — each put a moving part into every build for a state that `rm -rf web/dist
# && ln -s <main>/web/dist web/dist` restores. A worktree that needs a frontend of its own builds
# it before its first cargo build, in a target of its own (scripts/worktree.sh new --own-target).
#
# Sourced by check.sh, package-linux.sh, package-windows.sh, e2e.sh (--build) and worktree.sh
# (check, as a warning); defines functions only.

# shellcheck source=lanes.sh
source "$(dirname "${BASH_SOURCE[0]}")/lanes.sh"

# Whether checkout $1 is in the trap: a linked worktree whose web/dist is a directory of its own,
# not the link, while it builds in the main checkout's target directory (rd_target_dir).
rd_web_dist_trap() {
    local root="$1" main
    main="$(rd_main_root "$root")"
    [[ "$main" != "$root" && -d "$root/web/dist" && ! -L "$root/web/dist" ]] || return 1
    [[ "$(rd_target_dir "$root")" == "$(rd_target_dir "$main")" ]]
}

# Whether linked worktree $1 has no web/dist at all — neither the link nor a folder (2026-10-08).
# check.sh's web half then builds a folder of its own (check-web.sh builds where there is no
# link) while the Rust half already compiles rd-api against a missing folder: the run fails on
# rust-embed, and the next one is refused as the trap above.
rd_web_dist_missing() {
    local root="$1" main
    main="$(rd_main_root "$root")"
    [[ "$main" != "$root" && ! -e "$root/web/dist" && ! -L "$root/web/dist" ]] || return 1
    [[ "$(rd_target_dir "$root")" == "$(rd_target_dir "$main")" ]]
}

# Refuses checkout $1 when it is in the trap, saying why and how out; $2 names the caller.
rd_web_dist_guard() {
    local root="$1" label="${2:-this run}" main
    main="$(rd_main_root "$root")"
    if rd_web_dist_missing "$root"; then
        echo "!! $label is refused: $root/web/dist is missing — neither the link to the main" >&2
        echo "   checkout's web/dist nor a folder. The web half would build a folder of its own while" >&2
        echo "   rd-api compiles against none (scripts/lib/web-dist.sh)." >&2
        echo "   Restore the link: ln -s '$main/web/dist' '$root/web/dist'" >&2
        return 1
    fi
    rd_web_dist_trap "$root" || return 0
    echo "!! $label is refused: $root/web/dist is a directory of its own, and this worktree" >&2
    echo "   builds in the main checkout's target ($(rd_target_dir "$root"))." >&2
    echo "   rust-embed keeps serving the web/dist it was first compiled against — the main" >&2
    echo "   checkout's — so the run would check that frontend, not this one (scripts/lib/web-dist.sh)." >&2
    echo "   Restore the link: rm -rf '$root/web/dist' && ln -s '$main/web/dist' '$root/web/dist'" >&2
    return 1
}
