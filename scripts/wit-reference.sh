#!/usr/bin/env bash
#
# Writes or checks the generated contract reference (RD-160-04): every world, interface,
# function, record, variant and enum of crates/rd-plugin-api/wit/rdownloader.wit, with its doc
# comments, as Markdown for the user wiki's plugin reference. The page carries it between
# `<!-- BEGIN wit-reference -->` and `<!-- END wit-reference -->`; nothing between the markers
# is edited by hand. The reader and the rules are in scripts/lib/wit-reference.py.
#
# A contract change runs this in the same change as the WIT, the template copies and the wiki
# page of the type it touches (AGENTS.md, Conventions).
#
# Usage:
#   scripts/wit-reference.sh --check                        # the WIT is read completely (CI)
#   scripts/wit-reference.sh --print                        # the Markdown to stdout
#   scripts/wit-reference.sh --wiki ~/projects/rdownloader.wiki           # write the page
#   scripts/wit-reference.sh --wiki ~/projects/rdownloader.wiki --check   # exit 1 when stale
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$ROOT/scripts/lib/wit-reference.py" "$ROOT" "$@"
