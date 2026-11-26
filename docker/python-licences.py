#!/usr/bin/env python3
"""Collects the licence texts of the Python packages the image installs, and refuses one nobody
looked at (RD-1480-12). Run by docker/Dockerfile with the interpreter of /opt/venv:

    python-licences.py <directory>

Writes the licence, copyright and NOTICE files every installed package carries to
<directory>/<package>/, and exits 1 naming each package whose declared licence is not one of
ALLOWED, or that carries no licence file. Asks no network: pip has recorded all of it.
"""

import importlib.metadata
import pathlib
import re
import sys

# What the image may contain, as the packages spell it: they run as programs of their own beside
# rDownloader, so a copyleft licence is fine here, and each entry stands for a package that has it.
ALLOWED = {
    "0BSD", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "GPL-2.0-only", "ISC", "LGPL-2.1-only",
    "MIT", "MIT-0", "MPL-2.0", "PSF-2.0", "Unlicense", "Zlib",
    # Older spellings: sortedcontainers; PySocks; pycryptodome.
    "Apache 2.0", "BSD", "BSD, Public Domain",
    # The classifier, where the field holds the text or nothing: isodate; trio-websocket.
    "License :: OSI Approved :: BSD License", "License :: OSI Approved :: MIT License",
}
LICENCE_FILE = re.compile(r"^(licen[cs]es?|copying|copyright|notice|unlicense|authors)", re.IGNORECASE)


def declared(metadata):
    """The licence a package declares: the SPDX expression, the one-line field, the classifier."""
    for value in (metadata.get("License-Expression"), metadata.get("License")):
        if value and "\n" not in value.strip() and not value.startswith("Copyright"):
            return value.strip()
    classifiers = [value for value in metadata.get_all("Classifier") or [] if value.startswith("License ::")]
    return " AND ".join(classifiers)


def is_allowed(expression):
    if expression in ALLOWED:
        return True
    parts = [part.strip(" ()") for part in re.split(r"\bAND\b", expression)]
    return all(any(choice.strip(" ()") in ALLOWED for choice in re.split(r"\bOR\b", part)) for part in parts)


def main(target):
    problems = []
    count = 0
    for distribution in importlib.metadata.distributions():
        name = re.sub(r"[-_.]+", "-", distribution.metadata["Name"]).lower()
        licence = declared(distribution.metadata)
        if not is_allowed(licence):
            problems.append(f"{name} {distribution.version}: licence {licence or 'not declared'}")
        files = [file for file in distribution.files or []
                 if ".dist-info/" in str(file) and LICENCE_FILE.match(file.name)]
        if not files:
            problems.append(f"{name} {distribution.version}: carries no licence file")
        for file in files:
            path = pathlib.Path(target, name, file.name)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(file.locate().read_bytes())
        count += 1
    for problem in problems:
        print(f"!! {problem}", file=sys.stderr)
    if not count:
        print("!! no package is installed for this interpreter", file=sys.stderr)
    if problems or not count:
        return 1
    print(f"    licence texts of {count} Python packages in {target}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
