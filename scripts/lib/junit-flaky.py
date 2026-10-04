#!/usr/bin/env python3
"""The flaky tests of nextest JUnit reports as GitHub annotations (RD-191-09 T15).

    junit-flaky.py <junit.xml>...

The `ci` profile retries a failed test once (.config/nextest.toml). A test that fails and then
passes keeps the run green — the owner's decision of 2026-10-04 — and nextest records it as a
`<testcase>` with `<flakyFailure>` (or `<flakyError>`) children. Each such test becomes one
`::warning` line naming the test, the platform (RUNNER_OS) and the run (GITHUB_RUN_ID and
GITHUB_RUN_ATTEMPT), so a flaky test is seen on the run's page instead of only in an artefact.
A missing or unreadable report is skipped with a notice. Always exits 0: an annotation, never a
failure.
"""

import os
import sys
import xml.etree.ElementTree as ET

FLAKY = ("flakyFailure", "flakyError")


def flaky_tests(path):
    root = ET.parse(path).getroot()
    for case in root.iter("testcase"):
        attempts = [child for child in case if child.tag in FLAKY]
        if attempts:
            name = f"{case.get('classname', '')} {case.get('name', '')}".strip()
            message = (attempts[0].get("message") or attempts[0].get("type") or "").splitlines()
            yield name, len(attempts), message[0] if message else ""


def escape(text):
    # The workflow command format: %, CR and LF in the message; also : and , in properties.
    return text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def main(paths):
    platform = os.environ.get("RUNNER_OS", "local")
    run = os.environ.get("GITHUB_RUN_ID", "-")
    attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "1")
    count = 0
    for path in paths:
        try:
            found = list(flaky_tests(path))
        except (OSError, ET.ParseError) as error:
            print(f"::notice title=JUnit report not read::{escape(f'{path}: {error}')}")
            continue
        for name, failures, first in found:
            count += 1
            detail = f" — first failure: {first}" if first else ""
            print(f"::warning title=Flaky test ({platform})::"
                  + escape(f"{name} failed {failures}x and passed on retry, on {platform}, "
                           f"run {run} attempt {attempt}{detail}"))
    print(f"{count} flaky test(s) in {len(paths)} report(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
