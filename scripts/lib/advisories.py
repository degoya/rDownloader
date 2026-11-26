"""The questions of scripts/advisories.sh, which documents them.

    advisories.py <checkout> <OSV server> ask|list
"""

import json
import pathlib
import re
import sys
import urllib.error
import urllib.request

# OSV takes 1000 queries in one batch.
BATCH = 500
# An advisory with more pages than this is a server that does not end.
PAGES = 20


class NoAnswer(Exception):
    """OSV could not be asked: no finding, and no all-clear either."""


def packages(root):
    """`(ecosystem, name, version)` of both lock files, sorted."""
    found = set()
    for line in (root / "docker/requirements.txt").read_text(encoding="utf-8").splitlines():
        if match := re.match(r"([A-Za-z0-9][A-Za-z0-9._-]*)==([^\s;\\]+)", line):
            found.add(("PyPI", match[1], match[2]))
    section = None
    for line in (root / "web/pnpm-lock.yaml").read_text(encoding="utf-8").splitlines():
        if line and not line[0].isspace():
            section = line.rstrip(":")
        elif section == "packages" and (
                match := re.fullmatch(r"  (['\"]?)(@?[^@'\"]+)@([^'\"]+)\1:", line)):
            found.add(("npm", match[2], match[3]))
    return sorted(found)


def waivers(root):
    """`id -> reason` of scripts/advisories-allow.txt."""
    allowed = {}
    path = root / "scripts/advisories-allow.txt"
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        identity, _, reason = line.strip().partition(" ")
        if not reason.strip():
            raise SystemExit(f"{path}: {identity} has no reason")
        allowed[identity] = reason.strip()
    return allowed


def request(url, body=None):
    data = None if body is None else json.dumps(body).encode()
    call = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(call, timeout=60) as answer:
            return json.load(answer)
    except (OSError, ValueError) as error:
        raise NoAnswer(f"{url}: {error}") from error


def identifiers(server, asked):
    """`(package, advisory id)` pairs OSV names for the asked packages."""
    for start in range(0, len(asked), BATCH):
        pending = [(package, None) for package in asked[start:start + BATCH]]
        for _ in range(PAGES):
            if not pending:
                break
            queries = []
            for (ecosystem, name, version), token in pending:
                query = {"package": {"ecosystem": ecosystem, "name": name}, "version": version}
                queries.append(query | ({"page_token": token} if token else {}))
            results = request(f"{server}/v1/querybatch", {"queries": queries}).get("results")
            if not isinstance(results, list) or len(results) != len(pending):
                raise NoAnswer(f"{server}: the answer is not one result per package")
            following = []
            for (package, _), result in zip(pending, results):
                result = result or {}
                for vulnerability in result.get("vulns") or []:
                    yield package, vulnerability["id"]
                if token := result.get("next_page_token"):
                    following.append((package, token))
            pending = following
        if pending:
            raise NoAnswer(f"{server}: more than {PAGES} pages")


def main(argv):
    root, server, mode = pathlib.Path(argv[1]), argv[2], argv[3]
    asked = packages(root)
    counts = {each: sum(package[0] == each for package in asked) for each in ("PyPI", "npm")}
    if not all(counts.values()):
        print(f"advisories: nothing to ask for: {counts}", file=sys.stderr)
        return 2
    if mode == "list":
        for package in asked:
            print(*package)
        return 0

    allowed = waivers(root)
    used, findings = set(), []
    for (ecosystem, name, version), identity in identifiers(server, asked):
        details = request(f"{server}/v1/vulns/{identity}")
        if details.get("withdrawn"):
            continue
        waived = {identity, *details.get("aliases", [])} & allowed.keys()
        if waived:
            used |= waived
            continue
        summary = " ".join((details.get("summary") or details.get("details") or "").split())[:160]
        findings.append(f"{ecosystem} {name} {version}: {identity} {summary} https://osv.dev/{identity}")
    for finding in findings:
        print(f"!! {finding}")
    for identity in sorted(allowed.keys() - used):
        print(f"!! scripts/advisories-allow.txt: {identity} matches nothing any more")
    print(f"==> {counts['PyPI']} Python and {counts['npm']} npm packages, {len(findings)} finding(s), "
          f"{len(used)} waived")
    return 1 if findings or allowed.keys() - used else 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except (NoAnswer, KeyError) as error:
        print(f"advisories: no answer, nothing checked: {error}", file=sys.stderr)
        sys.exit(2)
