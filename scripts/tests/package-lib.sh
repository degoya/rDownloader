#!/usr/bin/env bash
#
# scripts/lib/package.sh (RD-191-09), the half scripts/package-linux.sh and package-windows.sh
# share: the arguments and profile, the workspace version, and the signed plugins laid into a
# package — every packageable one, the examples dropped, and none at all refused. In a scratch
# checkout with a stand-in scripts/build-plugins.sh; no cargo, no pnpm.
#
# check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/package-lib.sh
set -euo pipefail

ROOT_REAL="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT_REAL/scripts/tests/lib/expect.sh"

LIB="$ROOT_REAL/scripts/lib/package.sh"
ROOT="$SCRATCH/checkout"
mkdir -p "$ROOT/scripts" "$ROOT/dist/plugins"
cd "$ROOT"
printf '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "2.3.4-beta.1"\n\n[workspace.dependencies]\nx = { version = "9.9.9" }\n' \
    > Cargo.toml
printf '#!/usr/bin/env bash\nprintf "%%s\\n" alpha beta\n' > scripts/build-plugins.sh
chmod +x scripts/build-plugins.sh

# Each case in a subshell: the functions exit on a refusal.
# shellcheck source=../lib/package.sh
in_lib() { (set -euo pipefail; export ROOT; source "$LIB"; "$@"); }
# shellcheck disable=SC2154  # set by the library's functions
args() { rd_package_args "$@"; echo "$skip_web $profile"; }

expect "defaults: build the web UI, profile release" "0 release" "$(in_lib args)"
expect "--skip-web and --profile NAME" "1 release-test" "$(in_lib args --skip-web --profile release-test)"
expect "--profile=NAME" "0 release-test" "$(in_lib args --profile=release-test)"
expect "RD_PACKAGE_PROFILE is the default" "0 release-test" "$(RD_PACKAGE_PROFILE=release-test in_lib args)"
run_status in_lib args --profile dev
expect_status "an unknown profile is refused" 2
run_status in_lib args --sign
expect_status "an unknown argument is refused" 2

# shellcheck disable=SC2154
version_of() { rd_package_version; echo "$version"; }
expect "the workspace version, not a dependency's" "2.3.4-beta.1" "$(in_lib version_of)"
printf '[package]\nversion = "1.0.0"\n' > Cargo.toml
run_status in_lib version_of
expect_status "no workspace version is refused" 1

touch dist/plugins/alpha.rdplug dist/plugins/beta.rdplug dist/plugins/example-x.rdplug
run_status in_lib rd_package_plugins "$SCRATCH/out"
expect_status "every packageable plugin is laid in" 0
expect "the examples are dropped" "alpha.rdplug beta.rdplug" "$(ls "$SCRATCH/out/plugins" | paste -sd' ' -)"
touch "$SCRATCH/out/plugins/gone.rdplug"
rm dist/plugins/beta.rdplug
run_status in_lib rd_package_plugins "$SCRATCH/out"
expect_status "a package short of a plugin is refused" 1
expect_output "and says so" "1 packaged, but 2 plugins are packageable"
expect_true "the set is replaced, not merged" '[[ ! -e "$SCRATCH/out/plugins/gone.rdplug" ]]'
rm dist/plugins/*.rdplug
run_status in_lib rd_package_plugins "$SCRATCH/empty"
expect_status "a package without any plugin is refused" 1
expect_output "naming the way out" "run scripts/build-plugins.sh first"

finish_tests "package-lib"
