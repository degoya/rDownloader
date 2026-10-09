# shellcheck shell=bash
#
# What a changed path asks of a check, in one place (RD-1120-06). Until 1.12 the rule "a
# documentation-only change gets no build" stood three times — verified.sh (the --full and the
# GitHub greens), check-scope.sh (a branch run's docs_only) and scope.sh (check.sh --defer) — and
# none of the three knew .github/readme/: the screenshot README.md shows made the 1.11.0 release
# run --full again and dispatch GitHub on all three platforms, 52 minutes, for a picture.
#
# Two questions, each answered here only:
#
#   * rd_inert_path — whether a path is read by no check at all: documentation (docs/, every
#     *.md), the README's pictures (.github/readme/, which nothing but README.md and the
#     documentation name) and the owner's private material (private/, which no build, test or
#     release step reads; RD-1230-03). crates/rd-core/recovery-matrix.md and crates/rd-api/mcp-coverage.md
#     are not: a test compares the first with rd_crash_points::CRASH_POINTS, rd-api's library
#     include_str!s the second.
#   * rd_paths_read_by — which of the paths on stdin a recorded green's half reads, so a green
#     still covers a tree that differs from its own only in what that half never looks at (audit
#     C1): the lints and the Rust tests read the Rust inputs, the web half web/ and extension/,
#     the preflight everything but the generators' output, plus the documentation its checks read.
#
# Sourced by verified.sh, scope.sh and lib/integrate.sh; defines functions and constants only.

RD_INERT_PATTERN='^docs/|\.md$|^\.github/readme/|^private/'
RD_NOT_INERT_PATTERN='^crates/rd-core/recovery-matrix\.md$|^crates/rd-api/mcp-coverage\.md$'

# The documentation the preflight reads all the same (PIPE-04), so a preflight green does not cover
# a change to it: the job layout check reads the job files, the two release-note checks
# RELEASE-NOTES.md and every plugin's CHANGES.md, scripts/tests/workflow-shape.sh the documents
# that name the wasm-tools version. No build and no Rust or web test reads them, so for every other
# half they stay inert, and they are listed here rather than in RD_NOT_INERT_PATTERN.
RD_PREFLIGHT_DOCS_PATTERN='^docs/roadmap/jobs/|^RELEASE-NOTES\.md$|^plugins/[^/]+/CHANGES\.md$|^AGENTS\.md$|^docs/(development|architecture)\.md$|^sdk/README\.md$'

# What the Rust build and its tests read: the sources, the manifests and lock file, the toolchain,
# cargo's, nextest's and cargo-deny's configuration, the migrations — and every path
# scripts/lib/rust-test-inputs.map names for a crate (a catalogue compiled in, a script a test
# pins), which rd_rust_input_pattern adds.
RD_RUST_INPUT_PATTERN='^crates/|^plugins/|^Cargo\.(toml|lock)$|^rust-toolchain\.toml$|^\.cargo/|\.sql$|^\.config/nextest\.toml$|^deny\.toml$'

# What the web half of check.sh reads: the frontend, the extension and the script that builds it.
RD_WEB_INPUT_PATTERN='^web/|^extension/|^scripts/build-extension\.sh$'

# The generated files: nobody edits them by hand, scripts/integrate.sh writes them once after the
# last merge, and a merge conflict in one of them has no side to take (lib/integrate.sh).
RD_GENERATED_FILES=(
    web/openapi.json
    web/src/api/schema.d.ts
    crates/rd-api/mcp-coverage.md
    crates/rd-api/licenses/third-party.json
    web/components.d.ts
    web/auto-imports.d.ts
)

# Whether path $1 is read by no check at all.
rd_inert_path() {
    [[ "$1" =~ $RD_INERT_PATTERN && ! "$1" =~ $RD_NOT_INERT_PATTERN ]]
}

# The paths on stdin that are not inert, in their order, the exceptions last.
rd_non_inert_paths() {
    local paths
    paths="$(cat)"
    { grep -vE "$RD_INERT_PATTERN" <<< "$paths" || true
      grep -E "$RD_NOT_INERT_PATTERN" <<< "$paths" || true
    } | sed '/^$/d'
}

# The Rust inputs as one extended regular expression: RD_RUST_INPUT_PATTERN and every row of the
# Rust test inputs map that names a crate (a row with `-` names none). The map beside this file.
rd_rust_input_pattern() {
    local map="${RD_RUST_INPUTS_MAP_FILE:-$(dirname "${BASH_SOURCE[0]}")/rust-test-inputs.map}" pattern names
    local -a patterns=("$RD_RUST_INPUT_PATTERN")
    if [[ -f "$map" ]]; then
        while read -r pattern names; do
            [[ -n "$pattern" && "$pattern" != \#* && "$names" != - ]] || continue
            patterns+=("$pattern")
        done < "$map"
    fi
    (IFS='|'; printf '%s\n' "${patterns[*]}")
}

# Whether one of the paths on stdin is a Rust input.
rd_rust_input_touched() {
    grep -qE "$(rd_rust_input_pattern)"
}

# The paths on stdin that half $1 of a recorded green reads, inert ones never among them:
#   clippy, windows, rust  the Rust inputs (rd_rust_input_pattern)
#   web                    web/, extension/ and scripts/build-extension.sh
#   preflight              everything but the generated files, which the generators write after
#                          integrate.sh's preflight and check.sh --full checks once more, and the
#                          documentation RD_PREFLIGHT_DOCS_PATTERN names (PIPE-04)
# Any other half reads every path that is not inert.
rd_paths_read_by() {
    local all paths
    all="$(cat)"
    paths="$(rd_non_inert_paths <<< "$all")"
    if [[ "$1" == preflight ]]; then
        paths="$(printf '%s\n' "$paths"; grep -E "$RD_PREFLIGHT_DOCS_PATTERN" <<< "$all" || true)"
        paths="$(sed '/^$/d' <<< "$paths")"
    fi
    [[ -n "$paths" ]] || return 0
    case "$1" in
        clippy|windows|rust) grep -E "$(rd_rust_input_pattern)" <<< "$paths" || true ;;
        web) grep -E "$RD_WEB_INPUT_PATTERN" <<< "$paths" || true ;;
        preflight) grep -vxF -f <(printf '%s\n' "${RD_GENERATED_FILES[@]}") <<< "$paths" || true ;;
        *) printf '%s\n' "$paths" ;;
    esac
}
