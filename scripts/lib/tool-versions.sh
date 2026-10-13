# shellcheck shell=bash
#
# The development tools' versions, one source each (RD-1280-12). Rust is rust-toolchain.toml's
# `channel`, wasm-tools is WASM_TOOLS_VERSION in lib/plugin-stamp.sh (it decides the components'
# bytes); the rest are pinned here. Scripts source this file; a workflow cannot, so every copy in
# .github/ and sdk/ci/ is held to these values by `scripts/tool-versions.sh --check`, which
# `check.sh --preflight` and CI's `supply-chain` job run. docs/development.md ("Tool versions") is
# the one prose list, and the check holds it too.
#
#   source "$ROOT/scripts/lib/tool-versions.sh"
#   echo "$RD_NEXTEST_VERSION"
#   rd_rust_version "$ROOT"          # 1.99.0
#   rd_wasm_tools_pin "$ROOT"        # 1.261.0

# shellcheck disable=SC2034  # read by the scripts that source this file
RD_NEXTEST_VERSION="0.9.146"
# shellcheck disable=SC2034
RD_SQLX_CLI_VERSION="0.9.0"
# shellcheck disable=SC2034
RD_CARGO_XWIN_VERSION="0.23.1"
# CI downloads both linters and checks them against these SHA-256 (scripts/ci-tools.sh linters).
# shellcheck disable=SC2034
RD_SHELLCHECK_VERSION="0.11.0"
# shellcheck disable=SC2034
RD_SHELLCHECK_SHA256="8c3be12b05d5c177a04c29e3c78ce89ac86f1595681cab149b65b97c4e227198"
# shellcheck disable=SC2034
RD_ACTIONLINT_VERSION="1.7.12"
# shellcheck disable=SC2034
RD_ACTIONLINT_SHA256="8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8"

# The Rust toolchain checkout $1 pins: rust-toolchain.toml's `channel`.
rd_rust_version() {
    sed -n 's/^channel = "\(.*\)"$/\1/p' "$1/rust-toolchain.toml" | head -n 1
}

# The wasm-tools checkout $1 pins: WASM_TOOLS_VERSION in scripts/lib/plugin-stamp.sh.
rd_wasm_tools_pin() {
    sed -n 's/^WASM_TOOLS_VERSION="\(.*\)"$/\1/p' "$1/scripts/lib/plugin-stamp.sh" | head -n 1
}
