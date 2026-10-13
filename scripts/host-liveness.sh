#!/usr/bin/env bash
#
# Whether the hoster domains the plugins claim still exist (RD-1280-20).
#
#   scripts/host-liveness.sh [--json] [--out <file>] [plugin...]
#
# Reads `match_domains` from every resolver and crawler manifest under plugins/ (or the plugins
# named), skips wildcards (`*`, `*.example.com`: a debrid service claims every host, and a
# wildcard names no host to ask), and asks each remaining domain twice, in parallel:
#
#   DNS    `dig` for A, then AAAA. NXDOMAIN, or an answer without an address, is dead.
#   HTTPS  `curl -I https://<domain>/` with a short timeout. Any status below 500 is alive; a
#          redirect stays alive and names its target, which is how a sold or parked domain shows.
#
# Each domain ends as `alive`, `dead` or `unsure` (a resolver failure, a timeout, a refused
# connection, a TLS error or a 5xx: worth a look, not a verdict). The table goes to stdout, or with
# `--json` one JSON document: `{"date", "domains": [{"domain", "plugins", "status", "detail"}]}`.
# The exit status is 0 whatever the hosts answered; 1 when `dig` or `curl` is missing or no
# domain was found, 2 for a usage error.
#
# The result is also written as Markdown, date, domain, plugin and status per row, to
# docs/plugins-liveness.md on a run over every plugin, or to `--out`; that file is this script's
# output and never edited by hand. A run with plugins named and no `--out` writes nothing.
#
# It asks the network, so it never runs in check.sh or CI's checks: by hand, before a release
# (the dead hosts go into a job of the next milestone), and as the `workflow_dispatch` workflow
# .github/workflows/host-liveness.yml, which keeps both outputs as an artifact.
# RD_LIVENESS_JOBS (default 16) caps the parallel domains, RD_LIVENESS_TIMEOUT (default 8) the
# seconds one HTTPS request may take.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    echo "usage: scripts/host-liveness.sh [--json] [--out <file>] [plugin...]" >&2
    exit 2
}

json=0
out=""
names=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --json) json=1 ;;
        --out)
            [[ $# -ge 2 && -n "$2" ]] || usage
            out="$2"
            shift
            ;;
        -h | --help) usage ;;
        -*) echo "host-liveness: unknown argument: $1" >&2; usage ;;
        *) names+=("$1") ;;
    esac
    shift
done
if [[ -z "$out" && ${#names[@]} -eq 0 ]]; then
    out="$ROOT/docs/plugins-liveness.md"
fi
for tool in dig curl; do
    command -v "$tool" > /dev/null || {
        echo "host-liveness: $tool is required (dnsutils / bind-utils, curl)" >&2
        exit 1
    }
done

LIVENESS_PY="$(cat <<'PY'
import concurrent.futures
import datetime
import glob
import json
import os
import re
import subprocess
import sys
import tomllib

root, as_json, out = sys.argv[1], sys.argv[2] == "1", sys.argv[3]
named = sys.argv[4:]
jobs = int(os.environ.get("RD_LIVENESS_JOBS") or 16)
timeout = int(os.environ.get("RD_LIVENESS_TIMEOUT") or 8)
TYPES = ("resolver", "crawler")
ORDER = {"dead": 0, "unsure": 1, "alive": 2}
CURL_ERRORS = {6: "no address", 7: "connection refused", 28: "timeout", 35: "TLS handshake",
               51: "certificate name", 52: "empty reply", 56: "connection reset", 60: "certificate"}


def domains():
    """{domain: [plugin, ...]} over the resolver and crawler manifests, wildcards left out."""
    found = {}
    if named:
        manifests = [f"{root}/plugins/{name}/manifest.toml" for name in named]
    else:
        manifests = sorted(glob.glob(f"{root}/plugins/*/manifest.toml"))
    for path in manifests:
        name = os.path.basename(os.path.dirname(path))
        try:
            with open(path, "rb") as manifest:
                data = tomllib.load(manifest)
        except FileNotFoundError:
            sys.exit(f"host-liveness: plugins/{name}: no such plugin")
        if data.get("plugin_type") not in TYPES:
            continue
        for domain in data.get("match_domains", []):
            domain = domain.strip().lower().rstrip(".")
            if domain and "*" not in domain:
                found.setdefault(domain, []).append(name)
    return found


def run(command, limit):
    try:
        done = subprocess.run(command, capture_output=True, text=True, timeout=limit)
        return done.returncode, done.stdout
    except subprocess.TimeoutExpired:
        return None, ""


def dns(domain):
    """(verdict, detail): `ok`, `dead` or `unsure`."""
    for kind in ("A", "AAAA"):
        code, output = run(["dig", "+time=3", "+tries=2", "+noall", "+comments", "+answer", domain, kind], 20)
        status = re.search(r"status: ([A-Z]+)", output)
        if code != 0 or status is None:
            return "unsure", "dns: no answer from the resolver"
        if status.group(1) == "NXDOMAIN":
            return "dead", "dns: NXDOMAIN"
        if status.group(1) != "NOERROR":
            return "unsure", f"dns: {status.group(1)}"
        if re.search(rf"\sIN\s+{kind}\s", output):
            return "ok", ""
    return "dead", "dns: no address"


def https(domain):
    """(verdict, detail) of a HEAD request to the domain's root."""
    command = ["curl", "-sS", "-o", "/dev/null", "-I", "--connect-timeout", str(min(timeout, 5)),
               "--max-time", str(timeout), "-w", "%{http_code} %{redirect_url}", f"https://{domain}/"]
    code, output = run(command, timeout + 5)
    if code is None:
        return "unsure", "https: timeout"
    if code != 0:
        return "unsure", "https: " + CURL_ERRORS.get(code, f"curl exit {code}")
    status, _, target = output.strip().partition(" ")
    if not status.isdigit() or status == "000":
        return "unsure", "https: no status"
    if int(status) >= 500:
        return "unsure", f"https: {status}"
    host = re.match(r"https?://([^/:?#]+)", target)
    if host and host.group(1).lower() not in (domain, f"www.{domain}"):
        return "alive", f"https: {status} to {host.group(1).lower()}"
    return "alive", f"https: {status}"


def probe(domain):
    verdict, detail = dns(domain)
    if verdict != "ok":
        return verdict, detail
    return https(domain)


found = domains()
if not found:
    sys.exit("host-liveness: no domain to ask in the manifests read")
with concurrent.futures.ThreadPoolExecutor(max_workers=max(jobs, 1)) as pool:
    answers = dict(zip(found, pool.map(probe, found)))
rows = sorted(
    ({"domain": domain, "plugins": sorted(found[domain]), "status": answers[domain][0], "detail": answers[domain][1]}
     for domain in found),
    key=lambda row: (ORDER[row["status"]], row["plugins"], row["domain"]),
)
date = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
counts = {status: sum(row["status"] == status for row in rows) for status in ORDER}
summary = f"{len(rows)} domains: {counts['alive']} alive, {counts['dead']} dead, {counts['unsure']} unsure"

if as_json:
    print(json.dumps({"date": date, "domains": rows}, indent=2))
else:
    for row in rows:
        print(f"{row['status']:<7} {row['domain']:<40} {', '.join(row['plugins']):<20} {row['detail']}")
print(f"host-liveness: {summary}", file=sys.stderr)

if out:
    lines = [
        "# Plugin host liveness",
        "",
        f"Written by `scripts/host-liveness.sh` on {date} (UTC); never edited by hand. Every domain of",
        "a resolver or crawler manifest's `match_domains`, asked by DNS and an HTTPS `HEAD`: dead has no",
        "address, unsure needs a look (resolver failure, timeout, TLS error, 5xx), alive answered.",
        "",
        f"{summary}.",
        "",
        "| Date | Domain | Plugin | Status | Detail |",
        "| --- | --- | --- | --- | --- |",
    ]
    lines += [f"| {date} | {row['domain']} | {', '.join(row['plugins'])} | {row['status']} | {row['detail']} |" for row in rows]
    # The public export carries no docs/, and the workflow runs there.
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    with open(out, "w", encoding="utf-8") as document:
        document.write("\n".join(lines) + "\n")
PY
)"

python3 -c "$LIVENESS_PY" "$ROOT" "$json" "$out" "${names[@]}"
