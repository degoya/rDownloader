#!/usr/bin/env bash
#
# Adds one translation key to all four locale catalogues at once.
#
# Every visible string has to exist in de, en, es and fr. Doing that by hand means editing four
# JSON files and keeping their key order and formatting identical; forgetting one is caught by
# the locale test, but only after the fact.
#
# Usage:
#   scripts/i18n-key.sh <catalogue> <dotted.key> <de> <en> <es> <fr>
#
# Example:
#   scripts/i18n-key.sh plugins actions.enable Aktivieren Enable Activar Activer
#
# A dot separates groups. To put a dot *inside* one key, escape it -- and quote the argument so
# the shell leaves the backslash alone:
#
#   scripts/i18n-key.sh server 'codes.collector\.check_no_resolver' Kein Keine Ninguno Aucun
#
# That is how `server.json` wants its error codes: `codes` is flat, because a code is one stable
# identifier and not a path, and the interface looks it up whole. Until RD-120-24 the script
# could not express that at all -- `codes.collector.check_no_resolver` built three levels and hit
# nothing, in all four languages at once, so the locale comparison could not see it either. Six
# server codes reached the tree that way (see web/src/i18n/sourceKeys.test.ts).
#
# The script now also refuses to open a new group inside a group whose keys already carry dots,
# which is exactly the shape of `codes`. A mistake there is loud instead of silent.
#
# Environment:
#   RD_LOCALES_DIR  where the catalogues live (default web/src/locales). Only the tests set it.
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

[[ $# -eq 6 ]] || {
    echo "usage: scripts/i18n-key.sh <catalogue> <dotted.key> <de> <en> <es> <fr>" >&2
    exit 2
}

python3 - "$@" <<'PY'
import collections, json, pathlib, sys

import os, re

catalogue, key, *values = sys.argv[1:]
languages = ['de', 'en', 'es', 'fr']


def split_key(text):
    """Split on dots, except where a backslash escapes one."""
    segments, current, escaped = [], [], False
    for character in text:
        if escaped:
            current.append(character if character == '.' else '\\' + character)
            escaped = False
        elif character == '\\':
            escaped = True
        elif character == '.':
            segments.append(''.join(current))
            current = []
        else:
            current.append(character)
    if escaped:
        current.append('\\')
    segments.append(''.join(current))
    if any(segment == '' for segment in segments):
        raise SystemExit(f'empty segment in key {text!r}')
    return segments


segments = split_key(key)
locales = pathlib.Path(os.environ.get('RD_LOCALES_DIR', 'web/src/locales'))

for language, value in zip(languages, values):
    path = locales / language / f'{catalogue}.json'
    if not path.exists():
        raise SystemExit(f'no catalogue at {path}')
    data = json.loads(path.read_text(), object_pairs_hook=collections.OrderedDict)

    node = data
    walked = []
    for segment in segments[:-1]:
        child = node.get(segment)
        if child is None:
            # A group whose keys already carry dots is flat on purpose -- `server.json`'s `codes`
            # and `plugins.json`'s `incompatible.reason` are the two in the tree. Opening a
            # subgroup inside one resolves nowhere, and the four catalogues agree with each other
            # about it, so nothing downstream notices.
            #
            # The dot in an existing key is the signature, not "holds only strings": 407 ordinary
            # groups hold only strings too (`actions`, `labels`, ...), and a new subgroup under
            # any of them is legitimate. Refusing those would have traded a silent miss for a
            # loud false one.
            if any('.' in existing for existing in node) and all(
                not isinstance(value, dict) for value in node.values()
            ):
                where = '.'.join(walked) or catalogue
                depth = len(walked)
                literal = chr(92) + '.'
                suggestion = '.'.join(walked + [literal.join(segments[depth:])])
                raise SystemExit(
                    f'{language}: {where!r} holds only literal keys, so {segment!r} would open a '
                    f'group nothing resolves.\n'
                    f'         Escape the dots to write one key instead, quoted so the shell '
                    f'keeps the backslash:\n'
                    f"           scripts/i18n-key.sh {catalogue} '{suggestion}' <de> <en> <es> <fr>"
                )
            child = collections.OrderedDict()
            node[segment] = child
        elif not isinstance(child, dict):
            raise SystemExit(f'{language}: {segment!r} already holds a string, not a group')
        node = child
        walked.append(segment)

    leaf = segments[-1]
    if leaf in node:
        raise SystemExit(f'{language}: {key} already exists as {node[leaf]!r}')
    node[leaf] = value
    # Two spaces and a trailing newline: what every catalogue in the tree uses.
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n')
    print(f'  {language}: {".".join(segments)} = {value}')
PY

echo "==> added to all four catalogues"
