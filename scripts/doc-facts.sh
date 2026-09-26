#!/usr/bin/env bash
#
# Writes or checks the release facts the documentation repeats (RD-140-24): the workspace
# version and its date in docs/feature-list.md's header, the number of bundled plugins, and the
# plugin contract `rdownloader:plugin@X.Y.Z` in README.md, docs/ and sdk/README.md — each read
# from its source (Cargo.toml, plugins/*/manifest.toml, crates/rd-plugin-api/wit/rdownloader.wit),
# never from another document. The anchors and the rules are in scripts/lib/doc-facts.py.
#
# The release pipeline writes them in its `doc-facts` step and checks them again in `docs-gate`,
# so a release cannot leave them behind. A sentence reworded past its anchor is a refusal (exit
# 2), not a silent skip.
#
# Usage:
#   scripts/doc-facts.sh                               # write what is stale, name each change
#   scripts/doc-facts.sh --check                       # write nothing; exit 1 on a stale value
#   scripts/doc-facts.sh --wiki ~/projects/rdownloader.wiki [--check]   # the user wiki as well
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$ROOT/scripts/lib/doc-facts.py" "$ROOT" "$@"
