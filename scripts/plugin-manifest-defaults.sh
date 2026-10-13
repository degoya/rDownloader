#!/usr/bin/env bash
#
# Removes from the bundled plugin manifests what they need not state (RD-1280-18): a shared field
# with the value plugins/metadata.toml holds, and a [limits] field at the host's default. The
# rules are in scripts/lib/plugin-manifest-defaults.py. Rerunnable: a converted manifest stays as
# it is, and a manifest a merge brought back in its long form is converted by running it again.
#
# Usage:
#   scripts/plugin-manifest-defaults.sh                 # convert every manifest, name each one
#   scripts/plugin-manifest-defaults.sh mega katfile    # only these
#   scripts/plugin-manifest-defaults.sh --check         # write nothing; exit 1 on a stated default
#
# Removing lines from a manifest changes the packaged manifest, and for a resolver that embeds its
# manifest (`include_str!("../manifest.toml")`) the component: the plugins it converted then need a
# version raise before the next signed build (scripts/build-plugins.sh --list-unbumped).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$ROOT/scripts/lib/plugin-manifest-defaults.py" "$ROOT" "$@"
