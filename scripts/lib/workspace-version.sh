# shellcheck shell=bash
#
# The workspace version: `version` in Cargo.toml's [workspace.package], the one source
# scripts/set-version.sh writes every copy from. Anchored to that section, because several others
# have a `version` key. One function instead of the same two `sed`s in eight scripts (RD-191-09).
#
#   source "$ROOT/scripts/lib/workspace-version.sh"
#   rd_workspace_version < Cargo.toml
#   git show "$commit:Cargo.toml" | rd_workspace_version

# The workspace version in the Cargo.toml text on stdin; nothing when there is none.
rd_workspace_version() {
    sed -n '/^\[workspace\.package\]/,/^\[/p' | sed -n 's/^version = "\(.*\)"/\1/p' | head -n 1
}

# The copies of it, as paths or globs relative to the checkout: every file scripts/set-version.sh
# writes the version into (it refuses one it has no rule for) and `--check` holds to Cargo.toml.
# The one list: scripts/lib/verified.sh takes a bump that changed nothing but version lines in
# RD_VERSION_FILES as version-only, so the release does not run its tests again. It once kept a
# list of its own, which missed the SDK templates, and every release from 1.25.0 on ran test,
# clippy and web again for half an hour (RD-1270-09).
RD_VERSION_COPIES=(web/package.json extension/manifest.base.json web/openapi.json
    sdk/ci/plugin.yml sdk/ci/repository.yml 'sdk/templates/*/manifest.toml')
# shellcheck disable=SC2034  # read by the scripts that source this file
RD_VERSION_FILES=(Cargo.toml Cargo.lock "${RD_VERSION_COPIES[@]}")
