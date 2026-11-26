"""The SPDX document of scripts/release-sbom.sh, which documents it.

    release-sbom.py <checkout> <output> <epoch> <minimum crates> <minimum npm packages>
"""

import base64
import datetime
import hashlib
import json
import pathlib
import re
import sys
import tomllib
import urllib.parse

LICENCE_TOKEN = re.compile(r"\(|\)|[A-Za-z0-9][A-Za-z0-9.+-]*")
OPERATORS = {"AND", "OR", "WITH"}


def licence_expression(declared):
    """The SPDX expression of a declared licence, or NOASSERTION for anything that is none.

    Cargo's old `MIT/Apache-2.0` means OR. A `LicenseRef-` would need its text in the document.
    """
    if not declared:
        return "NOASSERTION"
    text = " OR ".join(part.strip() for part in declared.split("/"))
    tokens = LICENCE_TOKEN.findall(text)
    if "".join(tokens) != re.sub(r"\s", "", text) or "LicenseRef" in text:
        return "NOASSERTION"
    # Two names or two operators side by side are prose, not an expression.
    words = [token for token in tokens if token not in "()"]
    alternating = all((word in OPERATORS) == (at % 2 == 1) for at, word in enumerate(words))
    return text if len(words) % 2 == 1 and alternating else "NOASSERTION"


def crates(root, listed, not_shipped):
    """Every registry package of Cargo.lock that is not listed as shipping nowhere."""
    lock = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
    for package in lock["package"]:
        name, version = package["name"], package["version"]
        if "source" not in package or f"{name}@{version}" in not_shipped:
            continue
        yield {
            "kind": "cargo",
            "name": name,
            "version": version,
            "licence": listed.get((name, version)),
            "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
            "checksum": ("SHA256", package["checksum"]) if "checksum" in package else None,
            "purl": f"pkg:cargo/{name}@{urllib.parse.quote(version, safe='')}",
        }


def npm_integrities(root):
    """`(name, version) -> integrity` of every package of web/pnpm-lock.yaml.

    Read line by line, as scripts/licenses.sh does: the `packages:` maps of the two YAML documents
    hold a key `name@version` at two spaces and its `resolution: {integrity: ...}` at four.
    """
    found, section, key = {}, None, None
    for line in (root / "web/pnpm-lock.yaml").read_text(encoding="utf-8").splitlines():
        if line and not line[0].isspace():
            section, key = line.rstrip(":"), None
        elif section != "packages":
            continue
        elif match := re.fullmatch(r"  (['\"]?)(@?[^@'\"]+)@([^'\"]+)\1:", line):
            key = (match[2], match[3])
        elif key and (match := re.match(
                r"    resolution: \{integrity: ([a-z0-9]+-[A-Za-z0-9+/=]+)", line)):
            found[key] = match[1]
    return found


def npm(root, listed):
    integrities = npm_integrities(root)
    for (name, version), licence in listed.items():
        integrity = integrities.get((name, version))
        if integrity is None:
            print(f"release-sbom: {name}@{version} is listed but not in web/pnpm-lock.yaml; "
                  "scripts/licenses.sh renews the list", file=sys.stderr)
            continue
        algorithm, digest = integrity.split("-", 1)
        bare = name.rsplit("/", 1)[-1]
        yield {
            "kind": "npm",
            "name": name,
            "version": version,
            "licence": licence,
            "url": f"https://registry.npmjs.org/{name}/-/{bare}-{version}.tgz",
            "checksum": (algorithm.upper(), base64.b64decode(digest).hex()),
            "purl": "pkg:npm/" + urllib.parse.quote(name, safe="/") + "@" + version,
        }


def spdx_package(entry):
    identity = re.sub(r"[^A-Za-z0-9.-]", "-", f"{entry['kind']}-{entry['name']}-{entry['version']}")
    package = {
        "SPDXID": f"SPDXRef-{identity}",
        "name": entry["name"],
        "versionInfo": entry["version"],
        "downloadLocation": entry["url"],
        "filesAnalyzed": False,
        "licenseConcluded": "NOASSERTION",
        "licenseDeclared": licence_expression(entry["licence"]),
        "copyrightText": "NOASSERTION",
        "externalRefs": [{
            "referenceCategory": "PACKAGE-MANAGER",
            "referenceType": "purl",
            "referenceLocator": entry["purl"],
        }],
    }
    if entry["checksum"]:
        algorithm, value = entry["checksum"]
        package["checksums"] = [{"algorithm": algorithm, "checksumValue": value}]
    return package


def document(root, epoch, entries):
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    workspace = manifest["workspace"]["package"]
    version, repository = workspace["version"], workspace["repository"]
    digest = hashlib.sha256()
    for path in ("Cargo.lock", "web/pnpm-lock.yaml"):
        digest.update((root / path).read_bytes())
    created = datetime.datetime.fromtimestamp(epoch, datetime.timezone.utc)
    application = {
        "SPDXID": "SPDXRef-rdownloader",
        "name": "rdownloader",
        "versionInfo": version,
        "downloadLocation": f"{repository}/releases/tag/v{version}",
        "filesAnalyzed": False,
        "licenseConcluded": "NOASSERTION",
        "licenseDeclared": workspace["license"],
        "copyrightText": "NOASSERTION",
        "primaryPackagePurpose": "APPLICATION",
    }
    packages = sorted((spdx_package(entry) for entry in entries), key=lambda each: each["SPDXID"])
    return {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"rdownloader-{version}",
        "documentNamespace": f"{repository}/spdx/rdownloader-{version}-{digest.hexdigest()[:32]}",
        "creationInfo": {
            "created": created.strftime("%Y-%m-%dT%H:%M:%SZ"),
            "creators": [f"Tool: rdownloader-release-sbom-{version}"],
        },
        "packages": [application, *packages],
        "relationships": [
            {"spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES",
             "relatedSpdxElement": application["SPDXID"]},
            *({"spdxElementId": application["SPDXID"], "relationshipType": "DEPENDS_ON",
               "relatedSpdxElement": package["SPDXID"]} for package in packages),
        ],
    }


def main(argv):
    root, output = pathlib.Path(argv[1]), pathlib.Path(argv[2])
    epoch, least = int(argv[3]), {"cargo": int(argv[4]), "npm": int(argv[5])}
    third_party = json.loads(
        (root / "crates/rd-api/licenses/third-party.json").read_text(encoding="utf-8"))

    def listed(key):
        return {(each["name"], each["version"]): each["license"] for each in third_party[key]}

    entries = [*crates(root, listed("rust"), set(third_party["rust_not_shipped"])),
               *npm(root, listed("npm"))]
    for kind, minimum in least.items():
        count = sum(entry["kind"] == kind for entry in entries)
        if count < minimum:
            print(f"::error::the SBOM names {count} {kind} packages, at least {minimum} expected",
                  file=sys.stderr)
            return 1
        print(f"release-sbom: {count} {kind} packages")
    output.write_text(json.dumps(document(root, epoch, entries), indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
