# Shared by export-public.sh and export-wiki.sh (RD-130-23): the secret scanner, the identity
# that authors a public commit, and the local clone whose tree an export replaces.
#
# Sourced, never run. Every function exits the calling script on a refusal, with the reason.

# Sets GITLEAKS_BIN: $GITLEAKS, else `gitleaks` on PATH, else the copy the inventory downloaded.
rd_public_find_gitleaks() {
    GITLEAKS_BIN="${GITLEAKS:-}"
    if [[ -z "$GITLEAKS_BIN" ]]; then
        GITLEAKS_BIN="$(command -v gitleaks || true)"
    fi
    if [[ -z "$GITLEAKS_BIN" && -x /tmp/claude-1000/public-inventory/gitleaks ]]; then
        GITLEAKS_BIN=/tmp/claude-1000/public-inventory/gitleaks
    fi
    if [[ -z "$GITLEAKS_BIN" || ! -x "$GITLEAKS_BIN" ]]; then
        echo "gitleaks not found: put it on PATH or name it with GITLEAKS=<path>." >&2
        echo "Nothing is exported without the secret scan. Releases: https://github.com/gitleaks/gitleaks/releases" >&2
        exit 1
    fi
}

# Sets AUTHOR_NAME and AUTHOR_EMAIL from the git identity of the repository in $1.
rd_public_find_author() {
    AUTHOR_NAME="$(git -C "$1" config user.name || true)"
    AUTHOR_EMAIL="$(git -C "$1" config user.email || true)"
    if [[ -z "$AUTHOR_NAME" || -z "$AUTHOR_EMAIL" ]]; then
        echo "git user.name and user.email must be set in $1; they author the export" >&2
        exit 1
    fi
}

# Scans directory $1 from inside it, so that a .gitleaks.toml at its root applies.
rd_public_scan() {
    echo "==> gitleaks on the export"
    if ! (cd "$1" && "$GITLEAKS_BIN" dir . --no-banner --redact --exit-code 1); then
        echo "!! gitleaks found something in the export; nothing was committed." >&2
        echo "   A real secret is removed at the source. A fixture gets an entry in .gitleaks.toml." >&2
        exit 1
    fi
}

# Brings the clone $1 of $2 to the tip of branch $3, cloning it when missing. An empty remote
# — the very first export — leaves HEAD unborn on $3.
rd_public_prepare_clone() {
    local dir="$1" remote="$2" branch="$3"
    if [[ ! -d "$dir/.git" ]]; then
        [[ ! -e "$dir" ]] || { echo "$dir exists but is not a git clone" >&2; exit 1; }
        echo "==> cloning $remote into $dir"
        git clone "$remote" "$dir"
    fi
    if [[ -n "$(git -C "$dir" status --porcelain)" ]]; then
        echo "$dir has uncommitted changes; clean it up first — the export replaces its tree" >&2
        exit 1
    fi
    git -C "$dir" fetch --quiet --tags origin || { echo "cannot fetch $remote" >&2; exit 1; }

    if git -C "$dir" rev-parse --verify --quiet "refs/heads/$branch" > /dev/null; then
        git -C "$dir" checkout --quiet "$branch"
        if git -C "$dir" rev-parse --verify --quiet "refs/remotes/origin/$branch" > /dev/null; then
            git -C "$dir" merge --quiet --ff-only "origin/$branch" \
                || { echo "local $branch has diverged from origin/$branch" >&2; exit 1; }
        fi
    elif git -C "$dir" rev-parse --verify --quiet "refs/remotes/origin/$branch" > /dev/null; then
        git -C "$dir" checkout --quiet -b "$branch" "origin/$branch"
    else
        echo "    the repository has no $branch yet; this is its first commit"
        git -C "$dir" symbolic-ref HEAD "refs/heads/$branch"
    fi
}

# Replaces everything in clone $1 but .git with the tree in $2 and stages the result. What
# rsync --delete --exclude=/.git would do, without needing rsync: a file the export no longer
# has is deleted, not left behind.
rd_public_replace_tree() {
    echo "==> replacing the tree of $1"
    find "$1" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf -- {} +
    cp -a "$2"/. "$1"/
    git -C "$1" add --all
}
