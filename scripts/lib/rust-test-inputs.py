#!/usr/bin/env python3
"""Holds scripts/lib/rust-test-inputs.map against the Rust sources (RD-191-09).

    rust-test-inputs.py <repo> [<map>]

Every string literal in a tracked `crates/**/*.rs` that names a path — `include_str!("../x")`,
`include_bytes!(…)`, `root.join("web/src/locales")` — is resolved the three ways such a path is
meant: against the source file's directory (the include macros), against its crate's directory
(`CARGO_MANIFEST_DIR`) and against the repository root. The first that git tracks counts; one
under crates/ or plugins/ is a crate's own input and needs no row, and a literal that resolves to
nothing tracked (`scripts/inbox`, a generated `web/dist`) is not a repository file at all.

What is left must be matched by a row of the map that names the crate: for a file, the row's
regex matches it; for a directory, the regex matches at least one tracked file under it. A row
whose packages are `-` exempts what it matches. A literal with `..` after a name
(`downloads/../scripts`) is a path the test builds under a temp directory and is skipped. Prints
one line per path without such a row and exits 1; exits 0 silently when every one has its row.
"""

import os
import re
import subprocess
import sys

LITERAL = re.compile(r'"((?:\.\./)*[A-Za-z0-9_.][A-Za-z0-9_./{}-]*/[A-Za-z0-9_./{}-]*)"')
OWN = ("crates/", "plugins/")


def tracked(repo):
    out = subprocess.run(["git", "-C", repo, "ls-files"], capture_output=True, text=True, check=True)
    return out.stdout.splitlines()


def rows(map_path):
    result = []
    for line in open(map_path, encoding="utf-8"):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        pattern, *packages = line.split()
        result.append((re.compile(pattern), set(packages)))
    return result


def resolve(literal, source, crate_dir, files, directories):
    # `{platform}` and other format holes: the directory before the first hole.
    literal = literal.split("{", 1)[0].rstrip("/")
    for base in (os.path.dirname(source), crate_dir, ""):
        path = os.path.normpath(os.path.join(base, literal)) if literal else ""
        if path.startswith(".."):
            continue
        if path in files or path in directories:
            return path
    return None


def main(argv):
    if len(argv) < 2:
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    repo = argv[1]
    map_path = argv[2] if len(argv) > 2 else os.path.join(repo, "scripts/lib/rust-test-inputs.map")
    files = tracked(repo)
    file_set = set(files)
    directories = {os.path.dirname(f) for f in files}
    for path in list(directories):
        while path:
            path = os.path.dirname(path)
            directories.add(path)
    table = rows(map_path)
    problems = set()
    for source in files:
        if not (source.startswith("crates/") and source.endswith(".rs")):
            continue
        crate = source.split("/")[1]
        crate_dir = f"crates/{crate}"
        package = crate_package(repo, crate_dir) or crate
        text = open(os.path.join(repo, source), encoding="utf-8", errors="replace").read()
        for literal in LITERAL.findall(text):
            if re.search(r"[^./]/\.\./", literal):
                continue
            path = resolve(literal, source, crate_dir, file_set, directories)
            if path is None or path in ("", "crates", "plugins") or path.startswith(OWN):
                continue
            under = [path] if path in file_set else [f for f in files if f.startswith(path + "/")]
            if not any((packages >= {package} or packages == {"-"}) and any(p.search(f) for f in under)
                       for p, packages in table):
                problems.add(f"{source} reads {path} (\"{literal}\"): no row of {os.path.relpath(map_path, repo)} "
                             f"names {package} for it")
    for problem in sorted(problems):
        print(problem)
    return 1 if problems else 0


_packages = {}


def crate_package(repo, crate_dir):
    if crate_dir not in _packages:
        name = None
        try:
            section = None
            for line in open(os.path.join(repo, crate_dir, "Cargo.toml"), encoding="utf-8"):
                line = line.strip()
                if line.startswith("["):
                    section = line
                elif section == "[package]" and line.startswith("name"):
                    name = line.split("=", 1)[1].strip().strip('"')
                    break
        except OSError:
            pass
        _packages[crate_dir] = name
    return _packages[crate_dir]


if __name__ == "__main__":
    sys.exit(main(sys.argv))
