# Writes VERSION.txt into a package directory, so the version can be read without starting it.
#
# Usage (sourced):
#   rd_build_stamp <version>                              # before `cargo build`
#   rd_write_version_file <out-dir> <version> <platform>  # after it
#
# rd_build_stamp exports RD_BUILD_COMMIT and RD_BUILD_TIME. crates/rdownloader/build.rs compiles
# the same two variables into the binary, whose About page shows them (RD-130-12), so the
# package's VERSION.txt and the running service name one commit and one second — the build
# script only works them out itself when nobody exported them, which is a development build.
# A value already in the environment is kept.
#
# The commit is marked "-dirty" when the working tree differs from it, which a package built for
# testing between commits then says about itself.
#
# Except in a release. scripts/release-pipeline.sh builds the packages before its `commit` step,
# so the tree is always dirty there and the release commit does not exist yet: v1.2.1 shipped
# saying `deeb564f-dirty` (RD-130-17). With RD_RELEASE_VERSION set — the pipeline exports it
# from its version bump on — the line says what the package is instead:
# `Release-Build 1.2.1 (Basis deeb564f)`, the commit the release was built on.
rd_build_stamp() {
    local version="$1"
    if [[ -z "${RD_BUILD_COMMIT:-}" ]]; then
        RD_BUILD_COMMIT="$(git rev-parse --short=8 HEAD 2>/dev/null || echo unknown)"
        if [[ -n "${RD_RELEASE_VERSION:-}" ]]; then
            # A mismatch means the tree is not the one being released; saying so beats shipping
            # a package whose two version lines disagree.
            if [[ "$RD_RELEASE_VERSION" != "$version" ]]; then
                echo "RD_RELEASE_VERSION is $RD_RELEASE_VERSION, but the package is $version" >&2
                return 1
            fi
            RD_BUILD_COMMIT="Release-Build $version (Basis $RD_BUILD_COMMIT)"
        elif ! git diff --quiet HEAD -- 2>/dev/null; then
            RD_BUILD_COMMIT="$RD_BUILD_COMMIT-dirty"
        fi
    fi
    RD_BUILD_TIME="${RD_BUILD_TIME:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}"
    export RD_BUILD_COMMIT RD_BUILD_TIME
}

rd_write_version_file() {
    local out="$1" version="$2" platform="$3"
    rd_build_stamp "$version"
    {
        echo "rDownloader $version"
        echo "commit   $RD_BUILD_COMMIT"
        echo "built    $RD_BUILD_TIME"
        echo "platform $platform"
    } > "$out/VERSION.txt"
    chmod 644 "$out/VERSION.txt"
}
