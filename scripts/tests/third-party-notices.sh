#!/usr/bin/env bash
#
# scripts/lib/third_party_notices.py and licence_policy.py on scratch packages (RD-1480-12): what
# counts as a licence file, what a package without one gets — the standard text and never a
# copyright line — that equal inputs give equal bytes, and which licences are refused. The committed
# THIRD-PARTY-NOTICES.txt names every component third-party.json lists beside the Rust test of it.
#
#   scripts/tests/third-party-notices.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

mkdir -p "$SCRATCH/alpha/src" "$SCRATCH/alpha/vendored/zlib" "$SCRATCH/alpha/tests/data" \
    "$SCRATCH/beta" "$SCRATCH/gamma"
printf 'MIT License\r\n\r\nCopyright (c) 2020 Alpha Authors   \r\n=======\r\n' > "$SCRATCH/alpha/LICENSE-MIT"
printf 'NOTICE of alpha\n' > "$SCRATCH/alpha/NOTICE"
printf 'zlib text\n' > "$SCRATCH/alpha/vendored/zlib/COPYING"
printf 'pub fn license() {}\n' > "$SCRATCH/alpha/src/license.rs"
printf 'a fixture\n' > "$SCRATCH/alpha/tests/data/LICENSE"
printf 'MIT License\n\nCopyright (c) 2020   Alpha Authors\n=======\n' > "$SCRATCH/beta/LICENSE"

cat > "$SCRATCH/render.py" <<'PY'
import pathlib, sys
root, scratch = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
sys.dont_write_bytecode = True
sys.path.insert(0, str(root / "scripts/lib"))
from third_party_notices import Component, render
licence = sys.argv[3] if len(sys.argv) > 3 else "MIT OR Apache-2.0"
components = [
    Component("gamma", "3.0.0", licence, "https://example.test/gamma", directory=scratch / "gamma",
              authors=("Gamma Person",)),
    Component("beta", "2.0.0", "MIT", "https://example.test/beta", directory=scratch / "beta"),
    Component("alpha", "1.0.0", "MIT", "https://example.test/alpha", directory=scratch / "alpha"),
]
try:
    sys.stdout.write(render("A preamble.\n", [("Crates", "What is linked.", components)],
                            root / "crates/rd-api/licenses/texts"))
except LookupError as error:
    sys.exit(f"no standard text of {', '.join(error.args[0])}")
PY
run_status python3 "$SCRATCH/render.py" "$ROOT" "$SCRATCH"
expect_status "rendering three packages" 0
first="$output"
expect_output "alpha with its licence and its source" "alpha 1.0.0 (MIT)"
expect_output "the source of each" "    source: https://example.test/beta"
expect "the packages are sorted by name" "alpha beta gamma" \
    "$(grep -oE '^(alpha|beta|gamma) [0-9]' <<< "$first" | cut -d' ' -f1 | paste -sd' ')"
expect_output "the NOTICE file is carried" "NOTICE of alpha"
expect_output "and a licence file of vendored code" "vendored/zlib/COPYING"
expect "a source file named license is none" 0 "$(grep -c 'pub fn license' <<< "$first" || true)"
expect "a test's fixture is none" 0 "$(grep -c 'a fixture' <<< "$first" || true)"
expect "a text two packages share is printed once" 1 "$(grep -c '^Copyright (c) 2020' <<< "$first")"
expect "a line git would read as a conflict marker is not left one" 0 "$(grep -cE '^={7}$' <<< "$first" || true)"
expect "line ends and trailing blanks are gone" 0 "$(grep -cE $'\r|[ \t]$' <<< "$first" || true)"
expect_output "gamma says it has no licence file" "The package carries no licence file; the standard text of what it declares applies."
expect_output "and names its manifest's author as such" "Authors its manifest names: Gamma Person"
expect "no copyright line was made up for gamma" 0 "$(grep -ci 'copyright.*gamma' <<< "$first" || true)"
expect_output "the standard MIT text keeps its placeholders" "Copyright (c) <year> <copyright holders>"
expect_output "every licence of the expression has its text" "Apache-2.0"
marks="$(grep -oE '^    text:   \[[0-9a-f]{8}\]' <<< "$first" | grep -oE '\[[0-9a-f]+\]' | sort -u)"
printed="$(grep -oE '^\[[0-9a-f]{8}\]' <<< "$first" | sort -u)"
expect "every text named is printed, and no other" "$marks" "$printed"

run_status python3 "$SCRATCH/render.py" "$ROOT" "$SCRATCH"
expect_true "a second run gives the same bytes" '[[ "$output" == "$first" ]]'
run_status python3 "$SCRATCH/render.py" "$ROOT" "$SCRATCH" "Nobody-1.0"
expect_status "a licence without a standard text stops the run" 1
expect_output "naming it" "no standard text of Nobody-1.0"

cat > "$SCRATCH/policy.py" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
sys.dont_write_bytecode = True
sys.path.insert(0, str(root / "scripts/lib"))
from licence_policy import NPM_ONLY, allowed_licences, is_allowed
allowed = allowed_licences(root / "deny.toml")
for good in ("MIT", "MIT/Apache-2.0", "Apache-2.0 / MIT", "AGPL-3.0-only OR MIT",
             "(MIT OR Apache-2.0) AND Unicode-3.0", "Apache-2.0 WITH LLVM-exception"):
    assert is_allowed(good, allowed), good
for bad in ("", "GPL-2.0-only", "MIT AND AGPL-3.0-only", "(MIT", "MIT)", "MIT Apache-2.0", "OR MIT",
            "Apache-2.0 WITH Nobody-exception", "SEE LICENSE IN file", "OFL-1.1"):
    assert not is_allowed(bad, allowed), bad
assert is_allowed("OFL-1.1", allowed | NPM_ONLY)
PY
run_status python3 "$SCRATCH/policy.py" "$ROOT"
expect_status "the allowed licences are deny.toml's, read as SPDX expressions" 0

# The standard library's page, as rustc's documentation ships it.
cat > "$SCRATCH/std.py" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
sys.dont_write_bytecode = True
sys.path.insert(0, str(root / "scripts/lib"))
from third_party_notices import rust_standard_library
page = """<h2 id="short-version">Short version</h2>Dual-licensed.
<h2 id="out-of-tree-dependencies">Out-of-tree dependencies</h2>
<h3>📦 wasip2-1.0.2+wasi-0.2.9</h3><p><b>Authors:</b> A &#60;a@b.test&#62;</p><p><b>License:</b> MIT</p>
<details><summary><code>LICENSE-MIT</code></summary><pre>
Copyright &#34;A&#34;
</pre></details>"""
std, crate = rust_standard_library(page, "1.2.3", root / "crates/rd-api/licenses/texts")
assert (std.version, std.license) == ("1.2.3", "MIT OR Apache-2.0")
assert (crate.name, crate.version, crate.license) == ("wasip2", "1.0.2+wasi-0.2.9", "MIT"), crate
assert crate.texts == (("LICENSE-MIT", '\nCopyright "A"\n'),), crate.texts
try:
    rust_standard_library("<html>", "1.2.3", root / "crates/rd-api/licenses/texts")
except ValueError:
    pass
else:
    raise AssertionError("a page of another form is accepted")
PY
run_status python3 "$SCRATCH/std.py" "$ROOT"
expect_status "the standard library's crates and texts are read from rustc's page" 0

# Nothing in the module may reach a network: the registry question is licenses.sh's alone.
expect "the notices module imports nothing that asks a network" "" \
    "$(grep -nE 'urllib|http\.client|socket|requests|subprocess' "$ROOT/scripts/lib/third_party_notices.py" || true)"

finish_tests third-party-notices
