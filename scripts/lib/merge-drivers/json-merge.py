#!/usr/bin/env python3
"""The `rd-json` merge driver for the locale catalogues (RD-1100-13).

.gitattributes names it for web/src/locales/**/*.json and scripts/integrate.sh registers it. Two
branches that each add a key with scripts/i18n-key.sh both append inside the same object, and git
calls that a conflict as text; as JSON it is two new keys. The merge is three-way, key by key and
nested: a key one side changed and the other left as it was in the base takes the change, a key
one side added is added, a key one side deleted while the other left it alone is deleted. Our
order is kept and their new keys follow, the place scripts/i18n-key.sh would have put them; the
file is written as that script writes it, two spaces and a trailing newline.

A real conflict stays one: the same key given two different values, a key deleted on one side
and changed on the other, a group on one side that is a string on the other, or a side that is
not JSON. Then the file is merged as text by `git merge-file`, with conflict markers, for a
person.

    json-merge.py <base> <ours> <theirs> [<path>]    # git's %O %A %B %P

Writes the result into <ours>; exit 0 when merged, 1 on a conflict.
"""
import collections
import json
import subprocess
import sys

MISSING = object()


class Conflict(Exception):
    pass


def merge_value(base, ours, theirs, where):
    if ours == theirs:
        return ours
    if base == ours:
        return theirs
    if base == theirs:
        return ours
    if isinstance(ours, dict) and isinstance(theirs, dict) and (base is MISSING or isinstance(base, dict)):
        return merge_object(base if isinstance(base, dict) else {}, ours, theirs, where)
    raise Conflict(f'{where or "the top level"} differs on both sides')


def merge_object(base, ours, theirs, path):
    merged = collections.OrderedDict()
    for key in list(ours) + [key for key in theirs if key not in ours]:
        where = f'{path}.{key}' if path else key
        value = merge_value(base.get(key, MISSING), ours.get(key, MISSING), theirs.get(key, MISSING), where)
        if value is not MISSING:
            merged[key] = value
    return merged


def load(path):
    with open(path, encoding='utf-8') as handle:
        text = handle.read()
    if not text.strip():
        # A file both sides added has an empty base.
        return collections.OrderedDict()
    value = json.loads(text, object_pairs_hook=collections.OrderedDict)
    if not isinstance(value, dict):
        raise Conflict('the top level is not an object')
    return value


def main(argv):
    if len(argv) < 4:
        print('usage: json-merge.py <base> <ours> <theirs> [<path>]', file=sys.stderr)
        return 2
    base_path, ours_path, theirs_path = argv[1:4]
    name = argv[4] if len(argv) > 4 else ours_path
    try:
        merged = merge_object(load(base_path), load(ours_path), load(theirs_path), '')
    except (Conflict, ValueError) as error:
        print(f'rd-json: {name}: {error}; merged as text', file=sys.stderr)
        text = subprocess.run(
            ['git', 'merge-file', '-L', 'ours', '-L', 'base', '-L', 'theirs', ours_path, base_path, theirs_path],
            check=False,
        )
        return 0 if text.returncode == 0 else 1
    with open(ours_path, 'w', encoding='utf-8') as handle:
        handle.write(json.dumps(merged, ensure_ascii=False, indent=2) + '\n')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
