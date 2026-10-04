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
