# shellcheck shell=bash
#
# What scripts/package-linux.sh and scripts/package-windows.sh do alike (RD-191-09: 52 lines kept
# twice): their arguments, the version, the web UI and the signed plugins. Sourced after the
# lock and the `cd` to the checkout root; ROOT is the caller's. The functions set the caller's
# `skip_web`, `profile` and `version`.
#
#   rd_package_args "$@"         # --skip-web, --profile NAME; RD_PACKAGE_PROFILE is the default
#   rd_package_version           # the workspace version, or exit 1
#   rd_package_web               # type-check and build web/, or with --skip-web demand it current
#   rd_package_plugins <out>     # dist/plugins/*.rdplug into <out>/plugins, every one or exit 1

# shellcheck source=workspace-version.sh
source "$(dirname "${BASH_SOURCE[0]}")/workspace-version.sh"

rd_package_args() {
    skip_web=0
    # The cargo profile (RD-150-20): `release` for anything published, `release-test` for the
    # owner's test packages — the same optimisation without the single code unit and LTO, much
    # faster to build. VERSION.txt names the profile, so a test package cannot pass for a release
    # one.
    profile="${RD_PACKAGE_PROFILE:-release}"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --skip-web) skip_web=1 ;;
            --profile) profile="${2:?--profile needs a name}"; shift ;;
            --profile=*) profile="${1#--profile=}" ;;
            *) echo "unknown argument: $1" >&2; exit 2 ;;
        esac
        shift
    done
    case "$profile" in
        release|release-test) ;;
        *) echo "unknown profile: $profile (release or release-test)" >&2; exit 2 ;;
    esac
}

rd_package_version() {
    version="$(rd_workspace_version < Cargo.toml)"
    if [[ -z "$version" ]]; then
        echo "could not read the workspace version from Cargo.toml" >&2
        exit 1
    fi
}

# The web UI is embedded at compile time via rust-embed, so it has to exist before cargo runs.
rd_package_web() {
    if [[ "$skip_web" -eq 0 ]]; then
        echo "==> type-checking and building the web UI"
        # `run build` was `vue-tsc --build --force && vite build` until RD-120-25. The type check
        # is now its own script, so it has to be named here to keep the release chain checking
        # exactly what it checked before.
        pnpm --dir web run typecheck:full
        pnpm --dir web run build
    elif ! scripts/web-dist-stale.sh; then
        echo "--skip-web was given but web/dist is not current; drop the flag" >&2
        exit 1
    fi
}

# Signed plugins come from dist/plugins, which is what the signing step writes. Replaced as a set
# rather than merged, so a plugin dropped from the bundle does not linger in the package.
rd_package_plugins() {
    local out="$1" expected actual
    mkdir -p "$out/plugins"
    if compgen -G "dist/plugins/*.rdplug" > /dev/null; then
        rm -f "$out"/plugins/*.rdplug
        install -m 644 dist/plugins/*.rdplug "$out/plugins/"
        # The examples are not bundled (RD-150-20); a signed one from before then may still sit
        # in dist/plugins until the next build-plugins.sh run removes it.
        rm -f "$out"/plugins/example-*.rdplug
        echo "    plugins: $(find "$out/plugins" -maxdepth 1 -name '*.rdplug' | wc -l)"
        # A package silently short of a plugin looks fine until somebody misses the feature. Not
        # simply every manifest: a plugin demanding a newer application version than this build
        # cannot be packaged yet, and build-plugins.sh owns that rule.
        expected="$("$ROOT/scripts/build-plugins.sh" --list-packageable | wc -l)"
        actual="$(find "$out/plugins" -maxdepth 1 -name '*.rdplug' | wc -l)"
        if [[ "$expected" -ne "$actual" ]]; then
            echo "!! $actual packaged, but $expected plugins are packageable (build-plugins.sh --list-packageable)." >&2
            echo "   run scripts/build-plugins.sh — a missing one is usually never built here." >&2
            exit 1
        fi
    else
        echo "    plugins: dist/plugins holds no .rdplug — leaving the packaged set as it is" >&2
    fi
    # A package without plugins starts with no hoster, no account provider and no intake at all,
    # and nothing says why (1.5 test package, 2026-09-27): refused, not shipped.
    if ! compgen -G "$out/plugins/*.rdplug" > /dev/null; then
        echo "!! no signed plugin in $out/plugins — run scripts/build-plugins.sh first" >&2
        exit 1
    fi
}
